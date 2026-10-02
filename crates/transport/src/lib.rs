//! `pij-transport` — message transports behind the frozen `Transport` port.
//!
//! Adapters own wire quirks; routing policy stays in `pij-core`.

#![deny(missing_docs)]

mod uds;

pub use uds::UdsTransport;
