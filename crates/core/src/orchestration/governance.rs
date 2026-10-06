use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::SeatId;

/// A named project governed by one prime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Lowercase project slug.
    pub slug: String,
    /// Human description; absent for projects created before metadata was stored.
    pub description: Option<String>,
    /// Repository root; absent when not historically recorded.
    pub repo: Option<String>,
    /// Linked plan document.
    pub plan_path: Option<String>,
    /// Project's designated prime, independently of display roles.
    pub prime_id: Option<SeatId>,
    /// Seat that created the project.
    pub created_by: SeatId,
    /// Caller-supplied creation time.
    pub created_at: u64,
}

/// A partial project metadata update. Unsaid fields preserve durable values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectUpdate {
    /// Replace the description when supplied.
    pub description: Option<String>,
    /// Replace the repository root when supplied.
    pub repo: Option<String>,
    /// Set or explicitly clear the plan linkage.
    pub plan_path: Option<Option<String>>,
    /// Set or explicitly clear the project prime.
    pub prime_id: Option<Option<SeatId>>,
}

/// Durable state of a stream allocation; reservation is not Git creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamState {
    /// Allocation persisted, with no confirmed worktree creation.
    Reserved,
    /// Git creation succeeded.
    Created,
    /// Governance closed; the worktree remains untouched.
    Closed,
}

/// A durable stream allocation and its materialization state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stream {
    /// Canonical project:slug identity.
    pub id: String,
    /// Owning project.
    pub project: String,
    /// Reserved ordinal.
    pub ordinal: u32,
    /// Stream slug.
    pub slug: String,
    /// Reserved branch.
    pub branch: String,
    /// Reserved worktree path.
    pub worktree: PathBuf,
    /// Selected base ref.
    pub base_ref: String,
    /// Seat that allocated the stream.
    pub created_by: SeatId,
    /// Allocation time.
    pub created_at: u64,
    /// Current materialization/governance state.
    pub state: StreamState,
}

/// Descriptive write intent, not an authorization grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fence {
    /// Stable declaration id.
    pub id: String,
    /// Canonical stream identity.
    pub stream: String,
    /// Declared owned paths or patterns, in caller order.
    pub paths: Vec<String>,
    /// Shared paths requiring coordination, in caller order.
    pub shared: Vec<String>,
    /// Seat declaring the fence.
    pub declared_by: SeatId,
    /// Declaration time.
    pub declared_at: u64,
}

/// A baton whose lease serialises access to one shared resource.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonDefinition {
    /// Stable baton name.
    pub name: String,
    /// Human-readable protected resource or invariant.
    pub description: String,
    /// Machine-readable protected resource, if recorded.
    pub resource: Option<String>,
    /// Optional verification probe.
    pub probe: Option<String>,
    /// Repository to which the resource belongs, if recorded.
    pub repo: Option<String>,
    /// Seat that created it.
    pub created_by: SeatId,
    /// Caller-supplied creation time.
    pub created_at: u64,
}

/// Lifecycle of a request to hold a baton.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatonRequestState {
    /// Awaiting a grant.
    Requested,
    /// Linked to a granted lease.
    Granted,
    /// Holder returned the lease.
    Returned,
    /// Authorized actor reclaimed the lease.
    Reclaimed,
}

/// Immutable request evidence plus its lease lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonRequest {
    /// Stable request id.
    pub id: String,
    /// Requested baton.
    pub baton: String,
    /// Intended holder.
    pub requester: SeatId,
    /// Why the request needs exclusive access.
    pub purpose: String,
    /// Requested resource version, if applicable.
    pub pin: Option<String>,
    /// Evidence supporting the request.
    pub evidence: Option<String>,
    /// Request time.
    pub requested_at: u64,
    /// Current lifecycle; evidence is never rewritten by a grant or release.
    pub state: BatonRequestState,
}

/// A durable orchestration role assignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAssignment {
    /// Assigned seat.
    pub seat: SeatId,
    /// Role name.
    pub role: String,
    /// Seat that made the assignment.
    pub assigned_by: SeatId,
    /// Caller-supplied assignment time.
    pub assigned_at: u64,
}

/// The closed seat-role vocabulary every setter enforces (plan 166).
pub const SEAT_ROLES: [&str; 4] = ["prime", "pm", "worker", "pa"];

/// Roles a governor stamps on the placement call (spawn, link). `prime` is
/// designated through the prime flag, never placed.
pub const PLACEMENT_ROLES: [&str; 3] = ["pm", "worker", "pa"];

/// Refuse any role outside [`SEAT_ROLES`], naming the allowed set.
///
/// # Errors
/// A human-readable refusal reason listing every allowed role.
pub fn check_seat_role(role: &str) -> Result<(), String> {
    check_role_in(role, &SEAT_ROLES)
}

/// Refuse any role outside [`PLACEMENT_ROLES`], naming the allowed set.
///
/// # Errors
/// A human-readable refusal reason listing every allowed placement role.
pub fn check_placement_role(role: &str) -> Result<(), String> {
    check_role_in(role, &PLACEMENT_ROLES)
}

fn check_role_in(role: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&role) {
        Ok(())
    } else {
        Err(format!(
            "role `{role}` is not allowed here; allowed: {}",
            allowed.join(", ")
        ))
    }
}

/// A prime designation is separate from a display role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrimeState {
    /// The current prime.
    Current,
    /// Explicitly retired; original designation metadata remains available.
    Retired,
}

/// The fleet's single prime designation, including explicit retirement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimeDesignation {
    /// Designated prime.
    pub seat: SeatId,
    /// Seat that made the designation.
    pub designated_by: SeatId,
    /// Caller-supplied designation time.
    pub designated_at: u64,
    /// Current or retired designation.
    pub state: PrimeState,
}

/// Dispatch lifecycle; transport acceptance is not delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DispatchState {
    /// Persisted obligation, not yet confirmed delivered.
    Queued,
    /// Transport supplied a successful delivery receipt.
    Delivered,
    /// Recipient acknowledged the exact packet digest.
    Acked,
}

/// A recipient's immutable acknowledgement of exact packet bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchAcknowledgement {
    /// Resolved recipient.
    pub seat: SeatId,
    /// Digest matching the persisted packet.
    pub packet_sha256: String,
    /// Acknowledgement time.
    pub at: u64,
}

/// Successful daemon-verified nonce, model, process and session evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchCanary {
    /// Nonce correlated with the real dispatched packet and receipt.
    pub nonce: String,
    /// Observed model, not merely a requested model.
    pub model: String,
    /// Verification time.
    pub passed_at: u64,
    /// Resolved evaluator.
    pub evaluator: SeatId,
}

/// A durable packet-pointer dispatch and its actual receipt state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dispatch {
    /// Stable dispatch id.
    pub id: String,
    /// Issuer.
    pub from: SeatId,
    /// Assignee.
    pub to: SeatId,
    /// Packet path; legacy body is retained verbatim on migration.
    pub packet_path: String,
    /// Exact packet digest, unknown for legacy dispatches.
    pub packet_sha256: Option<String>,
    /// Stable linkage into existing delivery receipts, unknown for legacy rows.
    pub msg_id: Option<String>,
    /// Current lifecycle.
    pub state: DispatchState,
    /// Caller-supplied creation time.
    pub created_at: u64,
    /// Actual transport receipt time, never inferred from acknowledgement.
    pub delivered_at: Option<u64>,
    /// Assignee acknowledgement time, including legacy acknowledgements.
    pub acknowledged_at: Option<u64>,
    /// Exact acknowledgement evidence, absent on pre-digest legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack: Option<DispatchAcknowledgement>,
    /// Successful real canary evidence, absent until verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary: Option<DispatchCanary>,
}

/// The supported reasons to close an assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskCloseReason {
    /// Assignee claims completion; this is not parent verification.
    Done,
    /// Work was cancelled.
    Cancelled,
    /// Work failed.
    Failed,
    /// Another assignment superseded this one.
    Superseded,
}

/// A durable task assignment, independent of display status or verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAssignment {
    /// Stable assignment id.
    pub id: String,
    /// Assigned node.
    pub node_id: SeatId,
    /// Work description.
    pub task: String,
    /// Optional project linkage.
    pub project: Option<String>,
    /// Assigning seat.
    pub opened_by: SeatId,
    /// Assignment time.
    pub opened_at: u64,
    /// First closure time.
    pub closed_at: Option<u64>,
    /// First closure reason.
    pub close_reason: Option<TaskCloseReason>,
}

/// Plan linkage only; never grants native extension delivery capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAttestation {
    /// Attested seat.
    pub seat: SeatId,
    /// Linked plan id.
    pub plan_id: String,
    /// Resolved attester.
    pub attested_by: SeatId,
    /// Attestation time.
    pub attested_at: u64,
}
