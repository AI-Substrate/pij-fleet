//! The drift gate's own negative proof (ac-0003).
//!
//! A gate nobody has seen fail is a gate nobody knows works. The task asks for
//! "add a forbidden dep, watch it go red, revert" — that ritual happened once and
//! its transcript is in `assets/reports/drift-negative.md`, but a ritual is not a
//! regression test: it protects nothing tomorrow. These tests are the same proof
//! made permanent, by judging COMMITTED metadata fixtures with the pure `check`:
//!
//! * `drifted-metadata.json` — `tokio` shipped in `pij-core`, `mockall` in
//!   `pij-store`. Must be RED, with those exact violations.
//! * `clean-metadata.json`   — the same workspace without the two edges. Must be
//!   GREEN, which is what stops "everything is a violation" from passing as a
//!   working check.
//! * the LIVE workspace — must be GREEN, so the fixtures cannot drift away from
//!   the tree they claim to describe.
//!
//! **Regenerating the fixtures** (needed whenever the real crate graph changes,
//! because a snapshot of an old graph now reports its unused permissions as
//! `StaleRule` — which is the check working):
//!
//! ```bash
//! cargo metadata --no-deps --format-version 1 \
//!   | jq '{packages: [.packages[] | {name, id, dependencies: [.dependencies[] | {name, kind}]}],
//!          workspace_members, version: 1}' \
//!   > crates/testkit/fixtures/arch/clean-metadata.json
//! # then re-plant tokio in pij-core and mockall@dev in pij-store for drifted-metadata.json
//! ```

use pij_testkit::arch::{self, DepKind, Violation};

fn allowlist() -> arch::Allowlist {
    arch::allowlist().expect("the committed allow-list must parse")
}

#[test]
fn drifted_metadata_is_refused_with_the_reason_named() {
    let graph =
        arch::Graph::from_cargo_metadata(include_str!("../fixtures/arch/drifted-metadata.json"))
            .expect("fixture must be readable cargo metadata");

    let violations = arch::check(&graph, &allowlist());

    assert!(
        violations.contains(&Violation::ForbiddenExternal {
            crate_name: "pij-core".to_string(),
            dep: "tokio".to_string(),
            kind: DepKind::Normal,
        }),
        "tokio shipped in the functional core must be refused; got {violations:?}"
    );
    assert!(
        violations.contains(&Violation::BannedEverywhere {
            crate_name: "pij-store".to_string(),
            dep: "mockall".to_string(),
            kind: DepKind::Dev,
        }),
        "a mocking framework must be refused workspace-wide, even dev-only; got {violations:?}"
    );
    assert_eq!(
        violations.len(),
        2,
        "exactly the two planted edges should be refused, no collateral: {violations:?}"
    );

    // The message is the deliverable: a violation an agent cannot act on is a
    // failure that costs a round trip to interpret.
    let rendered = violations[0].to_string();
    assert!(
        rendered.contains("pij-core -> tokio") && rendered.contains("arch-allowlist.toml"),
        "the violation must name the edge and where to fix it: {rendered}"
    );
}

#[test]
fn clean_metadata_passes() {
    let graph =
        arch::Graph::from_cargo_metadata(include_str!("../fixtures/arch/clean-metadata.json"))
            .expect("fixture must be readable cargo metadata");

    assert_eq!(
        arch::check(&graph, &allowlist()),
        Vec::new(),
        "the same workspace without the planted edges must be green — a check that \
         refuses everything is not a check"
    );
}

#[test]
fn the_live_workspace_matches_the_ratified_graph() {
    let graph = arch::workspace_graph().expect("`cargo metadata` must run for the live workspace");
    let violations = arch::check(&graph, &allowlist());
    assert!(
        violations.is_empty(),
        "live crate graph has drifted from workshop 001 R2: {violations:?}"
    );
}

#[test]
fn a_permission_nobody_uses_is_reported_as_drift() {
    // The check used to be one-directional: it judged real edges against the list
    // but never the list against reality, so a REMOVED dependency stayed
    // pre-approved and could be reintroduced later without the reviewed line the
    // allow-list exists to force. Found in review, 2026-08-28.
    let graph =
        arch::Graph::from_cargo_metadata(include_str!("../fixtures/arch/clean-metadata.json"))
            .expect("fixture");

    let mut allowlist = allowlist();
    allowlist
        .crates
        .get_mut("pij-core")
        .expect("core is described")
        .external
        .push(arch::Rule {
            dep: "nobody-depends-on-this".to_string(),
            kind: DepKind::Normal,
        });

    let violations = arch::check(&graph, &allowlist);
    assert!(
        violations.contains(&Violation::StaleRule {
            crate_name: "pij-core".to_string(),
            dep: "nobody-depends-on-this".to_string(),
            kind: DepKind::Normal,
        }),
        "an unused permission must be reported: {violations:?}"
    );
    assert!(
        violations[0].to_string().contains("no longer a dependency"),
        "and the message must say what to do: {}",
        violations[0]
    );
}

#[test]
fn a_dev_only_rule_never_permits_a_shipped_edge() {
    // The kind dimension exists because "dev-only" written in a comment is not
    // enforcement. Promotion into [dependencies] is the dangerous direction.
    let dev_rule = arch::Rule {
        dep: "tokio".to_string(),
        kind: DepKind::Dev,
    };
    assert!(dev_rule.permits(DepKind::Dev));
    assert!(!dev_rule.permits(DepKind::Normal));

    let ship_rule = arch::Rule {
        dep: "serde".to_string(),
        kind: DepKind::Normal,
    };
    assert!(ship_rule.permits(DepKind::Normal));
    assert!(
        ship_rule.permits(DepKind::Dev),
        "cleared to ship implies cleared for tests"
    );
    assert!(
        !ship_rule.permits(DepKind::Build),
        "build scripts are a separate axis"
    );
}

#[test]
fn an_unknown_rule_suffix_fails_the_parse_rather_than_becoming_a_crate_name() {
    let err = toml::from_str::<arch::Allowlist>(
        "[crates.pij-core]\nexternal = [\"tokio@prod\"]\nwhy = \"x\"\n",
    )
    .expect_err("`@prod` is not a dependency kind");
    assert!(
        err.to_string().contains("@prod"),
        "the parse error must name the bad suffix: {err}"
    );
}
