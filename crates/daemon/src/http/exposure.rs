//! Where the daemon may listen beyond loopback (plan 164 rulings 2 and 5),
//! decided in ONE function so a later move to TLS changes one place.
//!
//! Loopback is always bound. A configured non-loopback address is a second
//! listener that [`check_bind`] may refuse; a refusal costs only that listener.
//!
//! A BRAKE: it can only refuse a bind. Removing it lets the daemon listen on
//! more addresses, never on different ones, and it decides nothing about what
//! a request may do once it arrives.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

/// How far an allowed listener reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// Reachable only from this machine.
    Loopback,
    /// Reachable on the tailnet (100.64.0.0/10 or fd7a:115c:a1e0::/48), whose
    /// WireGuard tunnel encrypts the plain-HTTP bearer keys in transit.
    Tailscale,
    /// Any other address, allowed only by an explicit `--insecure-bind`: keys
    /// cross that network in clear.
    Insecure,
}

/// The daemon's second, non-loopback listener. Loopback is always bound; this
/// says what became of the configured remote address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteListener {
    /// The configured address is loopback: there is no second listener.
    None,
    /// Serving the same router on `addr`.
    Listening {
        /// The bound remote address.
        addr: SocketAddr,
        /// How far it reaches.
        exposure: Exposure,
    },
    /// The bind rule refused the address; loopback only.
    Refused(String),
    /// The OS failed the bind; loopback only.
    Failed(String),
}

/// A listen address the daemon refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindRefusal {
    /// Non-loopback with no pairing: nothing remote may call, so nothing
    /// remote should be able to connect.
    Unpaired(SocketAddr),
    /// Non-loopback, non-Tailscale, and `--insecure-bind` was not passed.
    Insecure(SocketAddr),
}

impl fmt::Display for BindRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unpaired(addr) => write!(
                formatter,
                "refusing to bind {addr}: no machine is paired (<state-dir>/peers.toml absent or lists no [[peer]]), so this daemon accepts no remote calls and listens on loopback only"
            ),
            Self::Insecure(addr) => write!(
                formatter,
                "refusing to bind {addr}: the daemon speaks plain HTTP, so bearer keys would cross this network in clear. Bind a Tailscale address (100.64.0.0/10 or fd7a:115c:a1e0::/48), or pass --insecure-bind to accept that"
            ),
        }
    }
}

impl std::error::Error for BindRefusal {}

fn is_tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            first == 100 && second & 0xC0 == 64
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
        }
    }
}

/// May the daemon listen on `addr`?
///
/// # Errors
/// Any non-loopback address when `paired` is false; a non-loopback,
/// non-Tailscale address (including the unspecified 0.0.0.0 / ::) unless
/// `insecure` is set.
pub fn check_bind(addr: SocketAddr, paired: bool, insecure: bool) -> Result<Exposure, BindRefusal> {
    let ip = addr.ip();
    if ip.is_loopback() {
        return Ok(Exposure::Loopback);
    }
    if !paired {
        return Err(BindRefusal::Unpaired(addr));
    }
    if is_tailscale(ip) {
        return Ok(Exposure::Tailscale);
    }
    if insecure {
        return Ok(Exposure::Insecure);
    }
    Err(BindRefusal::Insecure(addr))
}

/// The boot banner line for an allowed listener. `Insecure` is deliberately loud.
pub fn boot_banner(addr: &SocketAddr, exposure: Exposure) -> String {
    match exposure {
        Exposure::Loopback => {
            format!("pij daemon listening on {addr} (loopback; local bearer key required)")
        }
        Exposure::Tailscale => format!(
            "pij daemon listening on {addr} (Tailscale; paired machines' keys accepted on federation routes only)"
        ),
        Exposure::Insecure => format!(
            "WARNING: --insecure-bind: pij daemon listening on {addr} over PLAIN HTTP; paired machines' bearer keys cross this network in clear"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{BindRefusal, Exposure, check_bind};

    fn addr(text: &str) -> SocketAddr {
        text.parse().expect("socket address")
    }

    #[test]
    fn loopback_is_always_allowed_and_needs_no_pairing() {
        for text in ["127.0.0.1:7461", "[::1]:7461"] {
            assert_eq!(check_bind(addr(text), false, false), Ok(Exposure::Loopback));
        }
    }

    #[test]
    fn without_a_pairing_no_non_loopback_bind_is_allowed_even_insecurely() {
        for text in ["100.100.1.2:7461", "0.0.0.0:7461", "192.168.1.5:7461"] {
            assert_eq!(
                check_bind(addr(text), false, true),
                Err(BindRefusal::Unpaired(addr(text)))
            );
        }
    }

    #[test]
    fn a_paired_daemon_binds_tailscale_and_nothing_else_by_default() {
        let table = [
            ("100.64.0.1:7461", Ok(Exposure::Tailscale)),
            ("100.127.255.254:7461", Ok(Exposure::Tailscale)),
            ("[fd7a:115c:a1e0::5]:7461", Ok(Exposure::Tailscale)),
            ("100.63.255.255:7461", Err(())),
            ("100.128.0.1:7461", Err(())),
            ("[fd7a:115c:a1e1::5]:7461", Err(())),
            ("0.0.0.0:7461", Err(())),
            ("[::]:7461", Err(())),
            ("192.168.1.5:7461", Err(())),
        ];
        for (text, expected) in table {
            let decided = check_bind(addr(text), true, false);
            match expected {
                Ok(exposure) => assert_eq!(decided, Ok(exposure), "{text}"),
                Err(()) => assert_eq!(decided, Err(BindRefusal::Insecure(addr(text))), "{text}"),
            }
        }
    }

    #[test]
    fn insecure_bind_is_an_explicit_opt_in_for_paired_daemons() {
        assert_eq!(
            check_bind(addr("0.0.0.0:7461"), true, true),
            Ok(Exposure::Insecure)
        );
        assert_eq!(
            check_bind(addr("100.70.0.1:7461"), true, true),
            Ok(Exposure::Tailscale)
        );
    }
}
