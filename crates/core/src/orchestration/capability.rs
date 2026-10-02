/// Governance role relevant to the orchestration capability boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrchestrationRole {
    /// May inspect state but may not mutate governance.
    ReadOnly,
    /// May execute both reads and mutations.
    Operator,
}

/// Complete orchestration command vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// List projects.
    ListProjects,
    /// List streams.
    ListStreams,
    /// Inspect leases.
    ListLeases,
    /// Resolve seat parent attribution.
    ResolveParent,
    /// Record a spawn mapping.
    RecordSpawn,
    /// Reconcile equivalent descriptors.
    ReconcileDescriptors,
    /// Create a project.
    CreateProject,
    /// Reserve and create a stream.
    CreateStream,
    /// Claim a baton lease.
    ClaimLease,
    /// Release a baton lease.
    ReleaseLease,
    /// Assign an orchestration role.
    AssignRole,
    /// Designate the prime.
    DesignatePrime,
    /// Dispatch work.
    Dispatch,
    /// Acknowledge a dispatch.
    AcknowledgeDispatch,
}

impl Operation {
    /// Exhaustive vocabulary used by the capability regression.
    pub const ALL: [Self; 14] = [
        Self::ListProjects,
        Self::ListStreams,
        Self::ListLeases,
        Self::ResolveParent,
        Self::RecordSpawn,
        Self::ReconcileDescriptors,
        Self::CreateProject,
        Self::CreateStream,
        Self::ClaimLease,
        Self::ReleaseLease,
        Self::AssignRole,
        Self::DesignatePrime,
        Self::Dispatch,
        Self::AcknowledgeDispatch,
    ];

    /// Whether this command changes durable or authoritative state.
    pub const fn mutates(self) -> bool {
        !matches!(
            self,
            Self::ListProjects | Self::ListStreams | Self::ListLeases | Self::ResolveParent
        )
    }
}

/// Token proving an operation passed the one capability chokepoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizedOperation {
    operation: Operation,
}

impl AuthorizedOperation {
    /// Authorized command.
    pub const fn operation(self) -> Operation {
        self.operation
    }
}

/// Read-only refusal with the discriminating role and operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRefusal {
    /// Refused command.
    pub operation: Operation,
    /// Actionable explanation.
    pub reason: String,
}

/// Tagged result of the capability gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityDecision {
    /// Operation may proceed; the private token proves it passed the gate.
    Allowed(AuthorizedOperation),
    /// Operation is outside the caller's role.
    Refused(CapabilityRefusal),
}

/// The sole role-capability boundary for orchestration commands.
///
/// Every caller enters through [`Self::execute`]; verb implementations consume
/// [`AuthorizedOperation`] rather than re-deriving role policy per command.
///
/// # Composition recipe
///
/// ```text
/// use pij_core::orchestration::OrchestrationService;
/// let orchestration = Arc::new(OrchestrationService::new());
/// // Add `orchestration` to daemon Services beside EventBus.
/// // It is composed from existing adapters, so there is no AdapterChoice::Real
/// // arm and no wave-0 `not_yet(...)` refusal to replace.
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct OrchestrationService;

impl OrchestrationService {
    /// Construct the pure capability service.
    pub const fn new() -> Self {
        Self
    }

    /// Authorize one command at the single boundary.
    pub fn execute(&self, role: OrchestrationRole, operation: Operation) -> CapabilityDecision {
        if role == OrchestrationRole::ReadOnly && operation.mutates() {
            return CapabilityDecision::Refused(CapabilityRefusal {
                operation,
                reason: format!(
                    "{operation:?} mutates orchestration state; role ReadOnly may inspect only"
                ),
            });
        }
        CapabilityDecision::Allowed(AuthorizedOperation { operation })
    }
}
