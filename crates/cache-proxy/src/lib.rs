//! The Copilot cache-proxy sidecar (plan 228; PRD 004, blueprint 003).
//!
//! `pij-rs cache-proxy` runs it on 127.0.0.1:7471 under its own LaunchAgent, so
//! a daemon bounce never touches it. It holds one named account's `gh` token in
//! memory, forwards a Copilot seat's Anthropic Messages requests to the
//! account's Copilot endpoint with a 1-hour prompt cache, and serves
//! `GET /__pij/status` and `POST /__pij/reload` for the daemon's spawn gate.
//! The daemon never holds the token (blueprint d-2).

pub mod accounts;
pub mod config;
pub mod proxy;
pub mod readiness;
pub mod rewrite;
pub mod serve;
pub mod status;
pub mod token;

pub use proxy::{DEFAULT_PORT, Proxy, Settings};
pub use serve::{RELOAD_PATH, STATUS_PATH, router, run, serve};
pub use status::{State, Status};
