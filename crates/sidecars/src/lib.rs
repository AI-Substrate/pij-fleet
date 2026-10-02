//! Queue-backed Telegram, background-command, and chore consumers.
//!
//! The crate composes the seven frozen ports with concrete edge adapters. It adds
//! no port: queue/spine carry durable intent and facts; HTTP, processes, and files
//! are the concrete edges owned here.

#![deny(missing_docs)]

mod common;
mod runtime;

pub mod background;
pub mod chore;
pub mod telegram;

pub use common::{LoopHandle, Processed};
pub use runtime::{SidecarHandles, Sidecars};
