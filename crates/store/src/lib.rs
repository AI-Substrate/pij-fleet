//! `pij-store` — persistence behind the Registry, Spine and Queue ports.
//!
//! SQLite through sqlx (workshop 001 R1), with the migrations compiled into the
//! binary and run at boot. The only crate in the workspace that speaks SQL: if
//! another crate needs data, it needs a port, not a connection.

#![deny(missing_docs)]

pub mod background;
pub mod claims;
pub mod community;
pub mod decisions;
pub mod dispatch;
pub mod fleet;
pub mod governance;
pub mod migrate;
pub mod orchestration;
pub mod outputs;
pub mod queue;
pub mod registry;
mod role_assertion;
pub mod role_requests;
pub mod spine;
mod stalls;
pub mod status;

pub use claims::{Claim, ClaimKey, DeliveryCommit, DeliveryNote, DeliveryRecord, ReleaseOutcome};
pub use community::{Assignment, AssignmentCommit, CommunityPointer};
pub use governance::GovernanceOutcome;
pub use migrate::{SCHEMA_VERSION, StorePool, open, require_current_schema, schema_version};
pub use orchestration::{
    BatonLease, DispatchAck, LeaseClaim, SpawnRecordOutcome, SqliteOrchestration, StreamReservation,
};
pub use outputs::{CommunityOutput, OUTPUT_KINDS};
pub use queue::SqliteQueue;
pub use registry::SqliteRegistry;
pub use role_assertion::{Placement, PlacementCommit};
pub use role_requests::{
    CloseOutcome, ClosedOrOpen, ClosedRoleRequest, LIMIT_CLAIM_LEASE_MS, LimitClaim, LimitStage,
    REQUESTS_LIST_LIMIT, ReplyMode, RequestState, RoleRequest, WITHDRAWN_EVENT, Withdrawal,
};
pub use spine::SqliteSpine;
