//! Composition root #2: the CLI.
//!
//! Each verb is a thin function from parsed args to an [`Envelope`] plus a
//! rendering. Two faces — human and `--json` — from ONE value, so they cannot
//! disagree about what happened.
//!
//! The daemon ships inside this same binary, which is why a stale CLI can never
//! meet a newer daemon on one machine: they are the same artifact.

#![deny(missing_docs)]
pub mod address;
pub mod anomalies;
pub mod bounce;
pub mod client;
pub mod decisions;
pub mod fleet_report;
pub mod governance;
pub mod lifecycle;
pub mod role;
pub mod roster;

pub use address::{AddressError, parse_destination, render_destination};
pub use client::{
    CallerContext, DaemonClient, FederatedRoster, IdentityRequest, Phonehome, Registration,
    ReviveRequest, SendRequest, SpawnRequest, StreamFrame, TailError, TailStream, setup_refusal,
};

use std::path::{Path, PathBuf};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, ErrorKind};

/// Platform default for daemon state when neither `--state-dir` nor
/// `PIJ_RS_STATE_DIR` is set. Environment override precedence is resolved once
/// by the binary composition root, not hidden inside this default.
pub fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(
        // No HOME is a real environment (some launchd and CI contexts). A temp
        // dir keeps the binary usable and says where state went.
        || std::env::temp_dir().join("pij-rs-no-home"),
    );
    home.join(".pij-rs")
}

/// Render an envelope for a human, or as the JSON it already is.
pub fn render<T: serde::Serialize>(envelope: &Envelope<T>, as_json: bool) -> String {
    if as_json {
        if let Some(raw) = &envelope.raw_json {
            return raw.clone();
        }
        return serde_json::to_string(envelope).unwrap_or_else(|error| error.to_string());
    }
    if !envelope.ok {
        return format!(
            "{}: FAILED — {}",
            envelope.command,
            envelope.meta.as_deref().unwrap_or("no reason given")
        );
    }

    let rendered = match envelope.data.as_ref() {
        Some(data) => format!(
            "{}: ok — {}",
            envelope.command,
            serde_json::to_string(data).unwrap_or_else(|error| error.to_string())
        ),
        None => format!("{}: ok", envelope.command),
    };
    match envelope.meta.as_deref() {
        Some(meta) => format!("{rendered} — {meta}"),
        None => rendered,
    }
}

/// Render one tagged stream frame for either output face.
pub fn render_frame(frame: &StreamFrame, as_json: bool) -> String {
    if as_json {
        return serde_json::to_string(frame).unwrap_or_else(|error| error.to_string());
    }
    match frame {
        StreamFrame::Event {
            machine,
            cursor,
            event,
        } => format!(
            "{machine}:{cursor} {}{} {}",
            event.kind,
            event
                .seat
                .as_ref()
                .map(|seat| format!(" seat={seat}"))
                .unwrap_or_default(),
            event.payload
        ),
        StreamFrame::PeerState {
            machine,
            state,
            retry_in_ms,
            dropped,
            reason,
        } => format!(
            "{machine}: peer={state:?}{}{}{}",
            retry_in_ms
                .map(|value| format!(" retry_in_ms={value}"))
                .unwrap_or_default(),
            dropped
                .map(|value| format!(" dropped={value}"))
                .unwrap_or_default(),
            reason
                .as_ref()
                .map(|value| format!(" reason={value}"))
                .unwrap_or_default()
        ),
    }
}

/// Stable process exit code for a daemon envelope.
pub fn exit_code<T>(envelope: &Envelope<T>) -> u8 {
    if envelope.ok {
        return 0;
    }
    match envelope.error {
        Some(ErrorKind::Refused) => 2,
        Some(ErrorKind::NotFound) => 3,
        Some(ErrorKind::Auth) => 4,
        // A reset cursor exits 5: the caller's own state is stale and the fix is
        // to resume, which is a different action from "retry" (1) or "refused"
        // (2). An exit code is a discriminator a script branches on, so it gets
        // the same treatment as the kind it renders.
        Some(ErrorKind::CursorReset) => 5,
        Some(ErrorKind::Skew | ErrorKind::Adapter) | None => 1,
    }
}

/// The config the SHIPPED daemon boots with: every port real, loopback only,
/// SQLite under the state directory.
///
/// This replaces `hello_config`, which selected `Config::default()` — all seven
/// adapters fake and an empty (in-memory) store. That was correct for wave 0's
/// hello daemon and quietly wrong from wave 1 onward: the binary a user actually
/// runs could not corroborate a registration (`FakeLiveness` observes no
/// process, so every claim carrying `(pid, proc_start)` is refused), and its
/// roster, queue and history lived in process memory and died with it.
///
/// u-extension found it trying to run the matrix against the shipped binary. The
/// tell is worth remembering: **the default was inherited from the wave where it
/// was true, and nothing re-asked once the real adapters existed.** Each wave
/// added an adapter and none of them owned this line.
///
/// `offline` keeps the all-fake wiring for the demo and for tests that want a
/// daemon touching nothing outside the process. It is opt-IN, because a default
/// that silently persists nothing is the more dangerous of the two.
pub fn daemon_config(bind_addr: &str, store_path: &Path, offline: bool) -> Config {
    if offline {
        return Config {
            bind_addr: bind_addr.to_string(),
            ..Config::default()
        };
    }
    Config {
        bind_addr: bind_addr.to_string(),
        store_path: store_path.display().to_string(),
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            liveness: AdapterChoice::Real,
            tmux: AdapterChoice::Real,
            harness: AdapterChoice::Real,
            session_status: AdapterChoice::Real,
            // Real and conditionally open: the spawn path stamps
            // `cross_session_inbound_accept: Some(true)` only when it emitted
            // Claude's explicit inbound-accept setting. Unstamped seats remain
            // closed before socket discovery or IO.
            transport: AdapterChoice::Real,
        },
        ..Config::default()
    }
}

#[cfg(test)]
mod tests {
    use pij_core::model::{Envelope, ErrorKind};

    use super::{exit_code, render};

    #[test]
    fn human_rendering_does_not_hide_a_success_warning() {
        let mut envelope = Envelope::ok("pij inbox", ["message"]);
        envelope.meta = Some("ack failed; message may be read again".to_string());

        assert_eq!(
            render(&envelope, false),
            "pij inbox: ok — [\"message\"] — ack failed; message may be read again"
        );
        assert!(render(&envelope, true).contains("\"meta\":\"ack failed"));
    }

    #[test]
    fn exit_code_contract_is_an_explicit_table() {
        let cases = [
            (Envelope::ok("pij ping", ()), 0),
            (Envelope::refused("pij send", ErrorKind::Refused, "no"), 2),
            (
                Envelope::refused("pij list", ErrorKind::NotFound, "gone"),
                3,
            ),
            (
                Envelope::refused("pij ping", ErrorKind::Auth, "wrong key"),
                4,
            ),
            (Envelope::refused("pij ping", ErrorKind::Skew, "newer"), 1),
            (
                Envelope::refused("pij ping", ErrorKind::Adapter, "offline"),
                1,
            ),
        ];

        for (envelope, expected) in cases {
            assert_eq!(exit_code(&envelope), expected, "{envelope:?}");
        }
    }
}
