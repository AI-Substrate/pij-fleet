use crate::model::{SeatDescriptor, SeatId};

/// A secondary descriptor field whose disagreement is retained as history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DescriptorField {
    /// Harness kind.
    Harness,
    /// Tmux pane (the TS descriptor called this the window attachment).
    Pane,
    /// Working directory.
    Folder,
    /// Requested model.
    Model,
    /// Model provider.
    Provider,
    /// Thinking effort.
    Effort,
}

/// A spawn written by this binary, including the caller that created it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRecord {
    /// Stable launch id.
    pub spawn_id: String,
    /// Seat that invoked the spawn.
    pub spawner: SeatId,
    /// Caller-supplied observation time in milliseconds.
    pub recorded_at: u64,
}

/// How a descriptor's parent was established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionSource {
    /// The seat declared its own parent at adoption.
    SelfDeclared,
    /// A spawn record written by this binary supplied the parent.
    SpawnRecord,
}

/// Parent attribution never silently substitutes `unknown` for missing proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentAttribution {
    /// A parent and the evidence source that established it.
    Resolved {
        /// Parent seat.
        parent: SeatId,
        /// Evidence source.
        source: AttributionSource,
    },
    /// No parent can be established; the reason names the absent evidence.
    Unknown {
        /// Actionable reason rather than an unqualified `unknown`.
        reason: String,
    },
}

/// One descriptor plus explicit freshness evidence from its caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedDescriptor {
    /// Observed descriptor.
    pub descriptor: SeatDescriptor,
    /// When the caller live-verified it. `None` means no freshness claim.
    pub verified_at: Option<u64>,
}

/// A deterministic descriptor merge chosen only from explicit freshness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePlan {
    /// Newest live-verified row to retain.
    pub survivor: SeatDescriptor,
    /// Older equivalent row to tombstone.
    pub alias: SeatDescriptor,
    /// Secondary disagreements retained in merge history.
    pub mismatches: Vec<DescriptorField>,
    /// Freshness used for the decision.
    pub verified_at: u64,
}

/// Result of comparing two descriptor rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileDecision {
    /// Identity evidence proves the rows describe different seats.
    Different,
    /// Both observations name the same registry row; no alias exists.
    SameRow,
    /// Equivalent, but freshness is absent or tied, so nothing may be merged.
    Undecided {
        /// Why choosing a survivor would be a guess.
        reason: String,
        /// Secondary disagreements still exposed to the caller.
        mismatches: Vec<DescriptorField>,
    },
    /// Equivalent and ordered by explicit live-verification evidence.
    Merge(Box<MergePlan>),
}

/// Whether two descriptors identify one seat.
///
/// Identity is either the complete `(pid, proc_start)` pair or a non-empty
/// `spawn_id`. A pid by itself is never compared because [`crate::model::ProcIdentity`]
/// makes the pair indivisible.
pub fn descriptors_equivalent(left: &SeatDescriptor, right: &SeatDescriptor) -> bool {
    let same_process = matches!((left.proc, right.proc), (Some(a), Some(b)) if a == b);
    let same_spawn = matches!(
        (&left.spawn_id, &right.spawn_id),
        (Some(a), Some(b)) if !a.is_empty() && a == b
    );
    same_process || same_spawn
}

/// Enumerate tolerant secondary disagreements without changing equivalence.
pub fn descriptor_mismatches(
    left: &SeatDescriptor,
    right: &SeatDescriptor,
) -> Vec<DescriptorField> {
    let mut fields = Vec::new();
    if left.harness != right.harness {
        fields.push(DescriptorField::Harness);
    }
    if left.pane != right.pane {
        fields.push(DescriptorField::Pane);
    }
    if left.folder != right.folder {
        fields.push(DescriptorField::Folder);
    }
    if left.model != right.model {
        fields.push(DescriptorField::Model);
    }
    if left.provider != right.provider {
        fields.push(DescriptorField::Provider);
    }
    if left.effort != right.effort {
        fields.push(DescriptorField::Effort);
    }
    fields
}

/// Resolve a parent from declared lineage or a persisted spawn record.
pub fn derive_parent(
    descriptor: &SeatDescriptor,
    spawn_record: Option<&SpawnRecord>,
) -> ParentAttribution {
    if let Some(parent) = &descriptor.parent {
        return ParentAttribution::Resolved {
            parent: parent.clone(),
            source: AttributionSource::SelfDeclared,
        };
    }

    let Some(spawn_id) = descriptor.spawn_id.as_deref().filter(|id| !id.is_empty()) else {
        return ParentAttribution::Unknown {
            reason: "missing descriptor.spawn_id and no self-declared parent; no spawn evidence can be looked up"
                .to_string(),
        };
    };
    let Some(record) = spawn_record else {
        return ParentAttribution::Unknown {
            reason: format!(
                "missing spawn record for descriptor.spawn_id `{spawn_id}`; spawner attribution is unavailable"
            ),
        };
    };
    if record.spawn_id != spawn_id {
        return ParentAttribution::Unknown {
            reason: format!(
                "spawn evidence mismatch: descriptor.spawn_id `{spawn_id}` but record is `{}`",
                record.spawn_id
            ),
        };
    }
    ParentAttribution::Resolved {
        parent: record.spawner.clone(),
        source: AttributionSource::SpawnRecord,
    }
}

/// Decide whether and how two descriptor rows reconcile.
pub fn plan_descriptor_reconciliation(
    left: VerifiedDescriptor,
    right: VerifiedDescriptor,
) -> ReconcileDecision {
    if left.descriptor.id == right.descriptor.id {
        return ReconcileDecision::SameRow;
    }
    if !descriptors_equivalent(&left.descriptor, &right.descriptor) {
        return ReconcileDecision::Different;
    }

    let mismatches = descriptor_mismatches(&left.descriptor, &right.descriptor);
    let (Some(left_at), Some(right_at)) = (left.verified_at, right.verified_at) else {
        return ReconcileDecision::Undecided {
            reason: "equivalent descriptors lack caller-supplied verified_at evidence; refusing to guess from row order"
                .to_string(),
            mismatches,
        };
    };
    if left_at == right_at {
        return ReconcileDecision::Undecided {
            reason: format!(
                "equivalent descriptors have tied verified_at={left_at}; refusing to choose a survivor"
            ),
            mismatches,
        };
    }

    let (survivor, alias, verified_at) = if left_at > right_at {
        (left.descriptor, right.descriptor, left_at)
    } else {
        (right.descriptor, left.descriptor, right_at)
    };
    ReconcileDecision::Merge(Box::new(MergePlan {
        survivor,
        alias,
        mismatches,
        verified_at,
    }))
}
