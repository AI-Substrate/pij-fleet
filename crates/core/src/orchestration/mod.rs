//! Pure governance decisions for projects, streams, batons, roles, and seats.
//!
//! IO remains in `pij-store` and the daemon. This module consumes facts and
//! returns decisions, keeping the seven frozen ports unchanged.

mod capability;
mod descriptor;
mod governance;
mod inventory;

pub use capability::{
    AuthorizedOperation, CapabilityDecision, CapabilityRefusal, Operation, OrchestrationRole,
    OrchestrationService,
};
pub use descriptor::{
    AttributionSource, DescriptorField, MergePlan, ParentAttribution, ReconcileDecision,
    SpawnRecord, VerifiedDescriptor, derive_parent, descriptor_mismatches, descriptors_equivalent,
    plan_descriptor_reconciliation,
};
pub use governance::{
    BatonDefinition, BatonRequest, BatonRequestState, Dispatch, DispatchAcknowledgement,
    DispatchCanary, DispatchState, Fence, PlanAttestation, PrimeDesignation, PrimeState, Project,
    ProjectUpdate, RoleAssignment, Stream, StreamState, TaskAssignment, TaskCloseReason,
};
pub use inventory::{
    RepoInventory, StreamPlan, StreamPlanError, mint_ordinal, plan_stream_creation,
    plan_stream_creation_at_ordinal,
};
