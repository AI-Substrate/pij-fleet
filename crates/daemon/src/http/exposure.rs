use std::net::SocketAddr;

/// Whether a bound daemon is reachable only from this machine or from the LAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// The listener is reachable only through a loopback address.
    Loopback,
    /// The listener is reachable beyond loopback; bearer keys are its access control.
    Lan,
}

/// Classify the address the listener actually bound.
pub fn exposure(addr: &SocketAddr) -> Exposure {
    if addr.ip().is_loopback() {
        Exposure::Loopback
    } else {
        Exposure::Lan
    }
}

/// The exact boot banner for an already-bound address.
///
/// LAN exposure is deliberately loud: pij supplies no TLS and bearer keys are
/// the only access control between the daemon and the local network.
pub fn boot_banner(addr: &SocketAddr) -> String {
    match exposure(addr) {
        Exposure::Loopback => {
            format!("pij daemon listening on {addr} (loopback; local bearer key required)")
        }
        Exposure::Lan => format!(
            "WARNING: pij daemon listening on {addr} beyond loopback; bearer keys are the only LAN access control"
        ),
    }
}
