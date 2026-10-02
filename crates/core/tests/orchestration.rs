use std::collections::BTreeSet;
use std::path::Path;

use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::orchestration::{
    AttributionSource, CapabilityDecision, DescriptorField, Operation, OrchestrationRole,
    OrchestrationService, ParentAttribution, ReconcileDecision, RepoInventory, SpawnRecord,
    StreamPlanError, VerifiedDescriptor, derive_parent, descriptors_equivalent, mint_ordinal,
    plan_descriptor_reconciliation, plan_stream_creation, plan_stream_creation_at_ordinal,
};

fn descriptor(id: &str) -> SeatDescriptor {
    SeatDescriptor::new(id, Harness::Omp, "/abs/work")
}

#[test]
fn equivalence_merge_is_the_complete_five_row_table() {
    let process = ProcIdentity {
        pid: 4242,
        proc_start: 9001,
    };

    let mut same_pair_left = descriptor("pair-left");
    same_pair_left.proc = Some(process);
    let mut same_pair_right = descriptor("pair-right");
    same_pair_right.proc = Some(process);

    let mut same_spawn_left = descriptor("spawn-left");
    same_spawn_left.spawn_id = Some("spawn-7".to_string());
    let mut same_spawn_right = descriptor("spawn-right");
    same_spawn_right.spawn_id = Some("spawn-7".to_string());

    let mut recycled_left = descriptor("old-process");
    recycled_left.proc = Some(process);
    let mut recycled_right = descriptor("new-process");
    recycled_right.proc = Some(ProcIdentity {
        pid: process.pid,
        proc_start: process.proc_start + 1,
    });

    let mut tolerant_left = same_pair_left.clone();
    tolerant_left.id = SeatId::from("tolerant-left");
    tolerant_left.model = Some("model-a".to_string());
    tolerant_left.effort = Some("low".to_string());
    let mut tolerant_right = same_pair_right.clone();
    tolerant_right.id = SeatId::from("tolerant-right");
    tolerant_right.model = Some("model-b".to_string());
    tolerant_right.effort = Some("high".to_string());
    tolerant_right.folder = "/abs/other".to_string();

    let different_left = descriptor("different-left");
    let different_right = descriptor("different-right");

    let rows = [
        ("same-pid-pair", same_pair_left, same_pair_right, true),
        ("same-spawn-id", same_spawn_left, same_spawn_right, true),
        ("recycled-pid", recycled_left, recycled_right, false),
        (
            "tolerant-secondary-mismatch",
            tolerant_left,
            tolerant_right,
            true,
        ),
        (
            "genuinely-different",
            different_left,
            different_right,
            false,
        ),
    ];

    let failures = rows
        .into_iter()
        .filter_map(|(name, left, right, expected)| {
            let actual = descriptors_equivalent(&left, &right);
            (actual != expected).then_some(format!("{name}: expected {expected}, got {actual}"))
        })
        .collect::<Vec<_>>();
    assert!(
        failures.is_empty(),
        "equivalence table failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn reconciliation_uses_explicit_freshness_and_records_secondary_mismatches() {
    let process = ProcIdentity {
        pid: 77,
        proc_start: 88,
    };
    let mut old = descriptor("old-alias");
    old.proc = Some(process);
    old.model = Some("old-model".to_string());
    old.folder = "/old".to_string();
    let mut fresh = descriptor("fresh-canonical");
    fresh.proc = Some(process);
    fresh.model = Some("new-model".to_string());
    fresh.folder = "/new".to_string();

    let decision = plan_descriptor_reconciliation(
        VerifiedDescriptor {
            descriptor: old.clone(),
            verified_at: Some(10),
        },
        VerifiedDescriptor {
            descriptor: fresh.clone(),
            verified_at: Some(11),
        },
    );
    let ReconcileDecision::Merge(plan) = decision else {
        panic!("expected merge")
    };
    assert_eq!(plan.survivor, fresh);
    assert_eq!(plan.alias, old);
    assert_eq!(
        plan.mismatches,
        vec![DescriptorField::Folder, DescriptorField::Model]
    );

    let undecided = plan_descriptor_reconciliation(
        VerifiedDescriptor {
            descriptor: plan.survivor.clone(),
            verified_at: None,
        },
        VerifiedDescriptor {
            descriptor: plan.alias.clone(),
            verified_at: Some(10),
        },
    );
    assert!(matches!(undecided, ReconcileDecision::Undecided { .. }));
}

#[test]
fn parent_attribution_names_each_evidence_path_and_each_absence() {
    let mut declared = descriptor("declared");
    declared.parent = Some(SeatId::from("pij-parent"));
    assert_eq!(
        derive_parent(&declared, None),
        ParentAttribution::Resolved {
            parent: SeatId::from("pij-parent"),
            source: AttributionSource::SelfDeclared,
        }
    );

    let mut spawned = descriptor("spawned");
    spawned.spawn_id = Some("spawn-9".to_string());
    let record = SpawnRecord {
        spawn_id: "spawn-9".to_string(),
        spawner: SeatId::from("pij-spawner"),
        recorded_at: 99,
    };
    assert_eq!(
        derive_parent(&spawned, Some(&record)),
        ParentAttribution::Resolved {
            parent: SeatId::from("pij-spawner"),
            source: AttributionSource::SpawnRecord,
        }
    );

    let ParentAttribution::Unknown { reason } = derive_parent(&descriptor("adopted"), None) else {
        panic!("missing spawn id must be stated")
    };
    assert!(reason.contains("descriptor.spawn_id"));

    let ParentAttribution::Unknown { reason } = derive_parent(&spawned, None) else {
        panic!("missing spawn record must be stated")
    };
    assert!(reason.contains("spawn record") && reason.contains("spawn-9"));
}

#[test]
fn ordinal_minting_unions_clone_worktrees_and_branch_heads() {
    let inventory = RepoInventory {
        clone: BTreeSet::from([7]),
        worktrees: BTreeSet::from([11]),
        branch_heads: BTreeSet::from([13]),
    };
    assert_eq!(mint_ordinal(&inventory), Ok(14));
    let plan = plan_stream_creation(
        "rust-port",
        "orchestration",
        Path::new("/tmp/pij-worktrees"),
        "main",
        &inventory,
    )
    .expect("plan");
    assert_eq!(plan.branch, "s014/orchestration");
    assert_eq!(
        plan.worktree,
        Path::new("/tmp/pij-worktrees/s014-orchestration")
    );
}

#[test]
fn explicit_stream_ordinal_preserves_existing_names_and_does_not_mutate_inventory() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .unwrap();
    let stream = &fixture["fixture_context"]["records"]["stream"];
    let ordinal = u32::try_from(stream["ordinal"].as_u64().unwrap()).unwrap();
    let inventory = RepoInventory {
        branch_heads: BTreeSet::from([ordinal + 2]),
        ..RepoInventory::default()
    };
    let before = inventory.clone();
    let plan = plan_stream_creation_at_ordinal(
        stream["project"].as_str().unwrap(),
        stream["slug"].as_str().unwrap(),
        Path::new("/tmp/pij-worktrees"),
        stream["base_ref"].as_str().unwrap(),
        &inventory,
        ordinal,
    )
    .unwrap();
    assert_eq!(plan.ordinal, ordinal);
    assert_eq!(
        plan.branch,
        format!("s{ordinal:03}/{}", stream["slug"].as_str().unwrap())
    );
    assert_eq!(
        plan.worktree,
        Path::new("/tmp/pij-worktrees").join(format!(
            "s{ordinal:03}-{}",
            stream["slug"].as_str().unwrap()
        ))
    );
    assert_eq!(inventory, before);
}

#[test]
fn explicit_stream_ordinal_refuses_collisions_in_each_git_namespace() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .unwrap();
    let stream = &fixture["fixture_context"]["records"]["stream"];
    let ordinal = u32::try_from(stream["ordinal"].as_u64().unwrap()).unwrap();
    for inventory in [
        RepoInventory {
            clone: BTreeSet::from([ordinal]),
            ..RepoInventory::default()
        },
        RepoInventory {
            worktrees: BTreeSet::from([ordinal]),
            ..RepoInventory::default()
        },
        RepoInventory {
            branch_heads: BTreeSet::from([ordinal]),
            ..RepoInventory::default()
        },
    ] {
        assert_eq!(
            plan_stream_creation_at_ordinal(
                stream["project"].as_str().unwrap(),
                stream["slug"].as_str().unwrap(),
                Path::new("/tmp/pij-worktrees"),
                stream["base_ref"].as_str().unwrap(),
                &inventory,
                ordinal,
            ),
            Err(StreamPlanError::OrdinalReserved { ordinal })
        );
    }
}

#[test]
fn read_only_capability_is_enforced_at_one_exhaustive_chokepoint() {
    let service = OrchestrationService::new();
    for operation in Operation::ALL {
        let decision = service.execute(OrchestrationRole::ReadOnly, operation);
        assert_eq!(
            matches!(decision, CapabilityDecision::Refused(_)),
            operation.mutates(),
            "read-only decision drifted for {operation:?}"
        );
        assert!(matches!(
            service.execute(OrchestrationRole::Operator, operation),
            CapabilityDecision::Allowed(_)
        ));
    }
}
