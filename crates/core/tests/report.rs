use std::cell::Cell;

use pij_core::error::PijError;
use pij_core::model::{CARD_LIMIT, Event, Harness, SeatDescriptor, SeatId, SemanticState};
use pij_core::ports::{Registry, Spine};
use pij_core::report::{CARD_EVENT_KIND, ReportConfig, ReportService};
use pij_testkit::block_on;
use pij_testkit::fakes::{FakeRegistry, FakeSpine};

fn field_with_collapsed_len(len: usize) -> String {
    assert!(len >= 2);
    format!("  {}\t\n  z  ", "x".repeat(len - 2))
}

fn registered(registry: &FakeRegistry, seat: &SeatId) {
    block_on(registry.put(SeatDescriptor::new(
        seat.as_str(),
        Harness::Omp,
        "/abs/worktree",
    )))
    .expect("register seat");
}

#[test]
fn card_fields_enforce_279_280_281_after_whitespace_collapse() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-boundary");
    let service = ReportService::new(&registry, &spine, || 100, ReportConfig::default());

    for len in [CARD_LIMIT - 1, CARD_LIMIT] {
        let input = field_with_collapsed_len(len);
        block_on(service.now(&seat, &input, "next")).expect("did at boundary must fit");
        block_on(service.now(&seat, "did", &input)).expect("next at boundary must fit");
        let status = block_on(service.card(&seat))
            .expect("read card")
            .expect("card exists");
        assert_eq!(status.card.next.chars().count(), len);
        assert!(
            status
                .card
                .next
                .chars()
                .all(|c| !c.is_whitespace() || c == ' ')
        );
    }

    for (did, next) in [
        (field_with_collapsed_len(CARD_LIMIT + 1), "next".to_string()),
        ("did".to_string(), field_with_collapsed_len(CARD_LIMIT + 1)),
    ] {
        assert_eq!(
            block_on(service.now(&seat, &did, &next)),
            Err(PijError::ReportTooLong {
                len: CARD_LIMIT + 1,
                limit: CARD_LIMIT,
            })
        );
    }

    let multibyte = "é".repeat(CARD_LIMIT);
    block_on(service.now(&seat, &multibyte, "next"))
        .expect("the contract counts characters, not UTF-8 bytes");
}

#[test]
fn absent_card_and_present_empty_card_are_different_facts() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-empty");
    let service = ReportService::new(&registry, &spine, || 42, ReportConfig::default());

    assert_eq!(block_on(service.card(&seat)).expect("read absent"), None);

    let seq = block_on(service.now(&seat, " \t\n ", "next")).expect("empty card is valid");
    let status = block_on(service.card(&seat))
        .expect("read present")
        .expect("card exists");
    assert_eq!(status.card.did, "");
    assert_eq!(status.card.next, "next");
    assert_eq!(status.card.at, 42);
    assert_eq!(status.card.seq, Some(seq));
}

#[test]
fn stale_is_strictly_past_the_configured_threshold_and_ignores_clock_reversal() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-stale");
    let now = Cell::new(1_000_u64);
    let service = ReportService::new(
        &registry,
        &spine,
        || now.get(),
        ReportConfig {
            stale_after_ms: 600,
        },
    );
    block_on(service.now(&seat, "did", "next")).expect("write card");

    now.set(1_599);
    let before = block_on(service.card(&seat)).expect("read").expect("card");
    assert_eq!((before.age_ms, before.stale), (599, false));

    now.set(1_600);
    let equal = block_on(service.card(&seat)).expect("read").expect("card");
    assert_eq!((equal.age_ms, equal.stale), (600, false));

    now.set(1_601);
    let after = block_on(service.card(&seat)).expect("read").expect("card");
    assert_eq!((after.age_ms, after.stale), (601, true));

    now.set(999);
    let reversed = block_on(service.card(&seat)).expect("read").expect("card");
    assert_eq!((reversed.age_ms, reversed.stale), (0, false));
}

#[test]
fn state_verbs_are_visible_correctable_clearable_and_independently_recorded() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-state");
    registered(&registry, &seat);
    let service = ReportService::new(&registry, &spine, || 100, ReportConfig::default());

    block_on(service.blocked(&seat, "  waiting   for db ")).expect("declare blocked");
    assert_eq!(
        block_on(registry.get(&seat))
            .expect("read registry")
            .expect("seat")
            .semantic_state,
        Some(SemanticState::Blocked)
    );
    let blocked = block_on(service.latest_state_record(&seat))
        .expect("read state event")
        .expect("state event");
    assert_eq!(blocked.state, Some(SemanticState::Blocked));
    assert_eq!(blocked.note.as_deref(), Some("waiting for db"));

    block_on(service.question(&seat, "which path?")).expect("correct to question");
    let question = block_on(service.latest_state_record(&seat))
        .expect("read question")
        .expect("question event");
    assert_eq!(question.state, Some(SemanticState::Question));
    assert_eq!(question.note.as_deref(), Some("which path?"));

    block_on(service.done(&seat)).expect("declare done");
    assert_eq!(
        block_on(registry.get(&seat))
            .expect("read registry")
            .expect("seat")
            .semantic_state,
        Some(SemanticState::Done)
    );

    block_on(service.clear(&seat)).expect("clear state");
    assert_eq!(
        block_on(registry.get(&seat))
            .expect("read registry")
            .expect("seat")
            .semantic_state,
        None
    );
    let cleared = block_on(service.latest_state_record(&seat))
        .expect("read clear")
        .expect("clear event");
    assert_eq!((cleared.state, cleared.note), (None, None));
}

#[test]
fn spine_failure_reports_that_state_landed_but_note_did_not() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-half-state");
    registered(&registry, &seat);
    spine.script_append_error("disk full while appending report.state");
    let service = ReportService::new(&registry, &spine, || 100, ReportConfig::default());

    let error = block_on(service.blocked(&seat, "waiting for storage"))
        .expect_err("history failure must be reported");
    assert_eq!(
        block_on(registry.get(&seat))
            .expect("read authoritative state")
            .expect("seat")
            .semantic_state,
        Some(SemanticState::Blocked),
        "Registry::put happens first and remains authoritative"
    );
    assert!(spine.is_empty(), "the failed note was not recorded");
    let message = error.to_string();
    assert!(message.contains("semantic state changed in the Registry"));
    assert!(message.contains("note and state history were not recorded"));
    assert!(message.contains("disk full while appending report.state"));
}

#[test]
fn malformed_latest_card_is_an_error_not_a_disappearing_card() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-malformed");
    block_on(spine.append(Event {
        seq: None,
        v: 1,
        at: 9,
        kind: CARD_EVENT_KIND.to_string(),
        seat: Some(seat.clone()),
        payload: "not-json".to_string(),
    }))
    .expect("seed malformed event");
    let service = ReportService::new(&registry, &spine, || 10, ReportConfig::default());

    let error = block_on(service.card(&seat)).expect_err("malformed card must be visible");
    assert!(
        error
            .to_string()
            .contains("could not decode report.now payload")
    );
}

/// `declare` preserves states without a dedicated named helper.
///
/// The registry row and the spine record are BOTH asserted: this service's whole
/// contract is that the authoritative state and its history move together.
#[test]
fn declare_reaches_the_states_the_named_helpers_never_exposed() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-declare");
    registered(&registry, &seat);
    let service = ReportService::new(&registry, &spine, || 100, ReportConfig::default());

    for state in [
        SemanticState::Ready,
        SemanticState::Waiting,
        SemanticState::Hold,
        SemanticState::Failed,
        SemanticState::Cancelled,
    ] {
        block_on(service.declare(&seat, Some(state), None, None, &[])).expect("declare");
        assert_eq!(
            block_on(registry.get(&seat))
                .expect("read registry")
                .expect("seat")
                .semantic_state,
            Some(state),
            "the registry is authoritative for the current state"
        );
        assert_eq!(
            block_on(service.latest_state_record(&seat))
                .expect("read history")
                .expect("a record")
                .state,
            Some(state),
            "and the history records the same declaration"
        );
    }

    // A note collapses exactly as the named helpers collapse theirs — one rule,
    // not a second one that happens to agree today.
    block_on(service.declare(
        &seat,
        Some(SemanticState::Blocked),
        Some("  on   a peer "),
        None,
        &[],
    ))
    .expect("declare with a note");
    assert_eq!(
        block_on(service.latest_state_record(&seat))
            .expect("read history")
            .expect("a record")
            .note
            .as_deref(),
        Some("on a peer")
    );

    // `None` clears, which is what `clear` is — the same operation, not a
    // parallel one.
    block_on(service.declare(&seat, None, None, None, &[])).expect("clear through declare");
    assert_eq!(
        block_on(registry.get(&seat))
            .expect("read registry")
            .expect("seat")
            .semantic_state,
        None
    );
}

#[test]
fn old_state_history_defaults_assignment_and_refs_without_losing_its_note() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-old-report");
    block_on(spine.append(Event {
        seq: None,
        v: 1,
        at: 9,
        kind: "report.state".into(),
        seat: Some(seat.clone()),
        payload: r#"{"state":"blocked","note":"original explanation","registry_seq":1}"#.into(),
    }))
    .expect("old event");
    let service = ReportService::new(&registry, &spine, || 10, ReportConfig::default());
    let record = serde_json::to_value(
        block_on(service.latest_state_record(&seat))
            .expect("read")
            .expect("state"),
    )
    .expect("serialize");
    assert_eq!(record["note"], "original explanation");
    assert!(record["assignment_id"].is_null());
    assert_eq!(record["refs"], serde_json::json!([]));
}

#[test]
fn assignment_state_round_trips_and_clear_drops_its_metadata() {
    let registry = FakeRegistry::new();
    let spine = FakeSpine::new();
    let seat = SeatId::from("pij-scoped-report");
    registered(&registry, &seat);
    let service = ReportService::new(&registry, &spine, || 100, ReportConfig::default());
    block_on(service.declare(
        &seat,
        Some(SemanticState::Waiting),
        None,
        Some("assignment-145"),
        &["x".into(), "y".into()],
    ))
    .expect("declare");
    let record = block_on(service.latest_state_record(&seat))
        .expect("read")
        .expect("record");
    assert_eq!(record.assignment_id.as_deref(), Some("assignment-145"));
    assert_eq!(record.refs, ["x", "y"]);
    pij_testkit::golden::assert_golden(
        "api/report-state-record.json",
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&record).expect("serialize")
        ),
    );
    block_on(service.clear(&seat)).expect("clear");
    let cleared = block_on(service.latest_state_record(&seat))
        .expect("read")
        .expect("record");
    assert!(cleared.assignment_id.is_none());
    assert!(cleared.refs.is_empty());
}
