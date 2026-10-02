//! `pij-testkit` — test substrate shipped as a crate, not copied per unit.
//!
//! * [`fakes`] — a deterministic, scriptable fake for each of the seven frozen
//!   ports. Doubles come from here or they do not exist.
//! * [`contract`] — the shared contract suites. One seam, one suite: the fake and
//!   the real adapter answer the same questions, so "it works with the fake" and
//!   "it works" cannot drift apart silently.
//! * [`fixtures`] — the addressable corpus of real captured pij traffic,
//!   malformed cases included, so every packet can cite a fixture by name.
//! * [`fresh_store`] — a real database per test, entropy-named and destroyed
//!   with the value, so no suite ever shares state by accident.
//! * [`golden`] — byte-goldens: the parity tripwire's mechanism, proven
//!   against captured bytes rather than a live, time-varying binary.
//! * [`exec`] — a ten-line executor, so core's tests await without a runtime.
//! * [`arch`] — the architecture drift gate.
//! * [`toolchain`] — the pin assert, stage 1 of `pij-gate`.
//! * [`lockfile`] — the reproducibility assert, stage 2.
//!
//! Never a dependency of production code: every consumer declares it `@dev`, and
//! the arch gate enforces the suffix.

#![deny(missing_docs)]

pub mod arch;
pub mod contract;
pub mod exec;
pub mod fakes;
pub mod fixtures;
pub mod fresh_store;
pub mod golden;
pub mod lockfile;
pub mod toolchain;

pub use exec::block_on;
pub use fresh_store::{FreshStore, fresh_dir};
