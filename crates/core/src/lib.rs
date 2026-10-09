//! `pij-core` — the functional core.
//!
//! Domain types, configuration, the error taxonomy, the eight ports, and
//! the pure logic over them. **No IO, no tokio, no SQL, no HTTP** — enforced
//! mechanically by `crates/testkit/arch-allowlist.toml`, not asserted here.
//!
//! Everything the daemon and CLI DECIDE is decided in this crate, which is what
//! lets those decisions be tested with zero doubles: give the function the facts,
//! check the verdict.

#![deny(missing_docs)]

pub mod address;
pub mod admission;
pub mod anomalies;
pub mod background;
pub mod cold_wake;
pub mod config;
pub mod control;
pub mod decisions;
pub mod delivery;
pub mod error;
pub mod events;
pub mod fleet;
pub mod framing;
pub mod fyi;
pub mod liveness;
pub mod model;
pub mod names;
pub mod orchestration;
pub mod ports;
pub mod report;
pub mod session_status;
pub mod status;
pub mod watchdog;
pub mod wire;

/// The daemon-owned virtual sender shared by job completions and death notices.
/// No registry row is required; clients may neither impersonate nor address it.
pub const BG_ACTOR: &str = "pij-bg";
