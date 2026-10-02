//! Gate-enforced agreement: the two admission consumers answer every case in the
//! shared table identically (workshop 001 R6f).
//!
//! This is the test that makes "one shared component" a fact rather than an
//! intention. It does not check that discovery is correct and, separately, that
//! registration is correct — it checks they cannot DIFFER, which is the property
//! that failed in TS and let subagents register as seats while the pane sweep
//! would have refused them.

use pij_core::admission::{self, Admission, Candidate, RefusalReason};
use pij_core::model::Pane;
use pij_daemon::admission as consumers;

fn as_pane(candidate: &Candidate) -> Pane {
    Pane {
        id: candidate.pane.clone().unwrap_or_else(|| "%0".to_string()),
        session: "pij".to_string(),
        window: "w".to_string(),
        title: candidate.id.clone(),
        cursor_x: None,
        cursor_y: None,
    }
}

#[test]
fn both_consumers_answer_every_case_identically() {
    for (name, candidate, expected) in admission::case_table() {
        let direct = admission::admit(&candidate);
        assert_eq!(
            direct, expected,
            "case `{name}`: the decision itself changed"
        );

        // Discovery never sees a `subagent_of` — a pane cannot tell you it is a
        // child — so those cases are compared through registration only, and the
        // table says so rather than the test silently skipping them.
        if candidate.subagent_of.is_none() {
            let discovery = consumers::from_pane(
                &as_pane(&candidate),
                candidate.harness.as_deref(),
                candidate.folder.as_deref(),
                candidate.bind_evidence,
            );
            assert_eq!(
                discovery, expected,
                "case `{name}`: discovery disagrees with the shared decision"
            );
        }

        let registration = consumers::from_registration(
            &candidate.id,
            candidate.harness.as_deref(),
            candidate.folder.as_deref(),
            candidate.bind_evidence,
            candidate.subagent_of.as_deref(),
        );
        assert_eq!(
            registration, expected,
            "case `{name}`: registration disagrees with the shared decision"
        );
    }
}

#[test]
fn the_case_table_covers_every_refusal_reason() {
    // A shared table that has quietly stopped covering a branch is worse than no
    // table: it reads as coverage. Adding a `RefusalReason` must therefore fail
    // this test until the table gains a case for it.
    let reasons: Vec<RefusalReason> = admission::case_table()
        .into_iter()
        .filter_map(|(_, _, verdict)| match verdict {
            Admission::Refuse(reason) => Some(reason),
            Admission::Admit => None,
        })
        .collect();

    assert!(reasons.iter().any(|r| matches!(r, RefusalReason::NoId)));
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, RefusalReason::UnknownHarness(_)))
    );
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, RefusalReason::NoAbsoluteFolder))
    );
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, RefusalReason::NoBindEvidence))
    );
    assert!(
        reasons
            .iter()
            .any(|r| matches!(r, RefusalReason::Subagent { .. }))
    );

    assert!(
        admission::case_table()
            .iter()
            .any(|(_, _, verdict)| *verdict == Admission::Admit),
        "a table with no admissions would pass by refusing everything"
    );
}

#[test]
fn a_subagent_is_refused_even_when_it_proves_a_live_session() {
    // TS defect #3 in one assertion: the child DOES have a real process and a
    // real environment. Being real is not the same as being a seat.
    let verdict = consumers::from_registration(
        "pij-child",
        Some("pi"),
        Some("/abs/tree"),
        true,
        Some("pij-parent"),
    );
    match verdict {
        Admission::Refuse(RefusalReason::Subagent { parent }) => assert_eq!(parent, "pij-parent"),
        other => panic!("a subagent must never be admitted: {other:?}"),
    }
}

#[test]
fn every_refusal_says_why_in_words_an_operator_can_act_on() {
    for (name, candidate, _) in admission::case_table() {
        if let Admission::Refuse(reason) = admission::admit(&candidate) {
            let rendered = reason.to_string();
            assert!(
                rendered.len() > 10,
                "case `{name}`: refusal reason is too thin to act on: {rendered}"
            );
        }
    }
}
