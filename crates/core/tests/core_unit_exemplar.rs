//! Tier-1 exemplar: pure core logic, **zero doubles**.
//!
//! Nothing here constructs a fake, because nothing here needs one. Give the
//! function the facts, check the verdict. Every decision that can be tested this
//! way should be, which is the whole reason the core crate performs no IO — and
//! it is why later units should be suspicious of any core test that reaches for
//! a double.

use pij_core::config::{AdapterChoice, Config};
use pij_core::error::PijError;
use pij_core::model::{
    ENVELOPE_VERSION, Envelope, Harness, SeatDescriptor, SeatId, SemanticState, SystemState,
};

#[test]
fn watchdog_eligibility_is_a_decision_table_not_a_timeout() {
    // TS defect #9: the watchdog nudged parked seats because it could only see
    // idleness, not INTENT. Each row below is a real incident class.
    let seat = |semantic: Option<SemanticState>, state: SystemState, relay: bool| {
        let mut descriptor = SeatDescriptor::new("pij-x", Harness::Pi, "/tmp");
        descriptor.semantic_state = semantic;
        descriptor.state = state;
        descriptor.relay = relay;
        descriptor
    };

    // Silent and idle with nothing declared, or declared available: the case the
    // watchdog exists for.
    assert!(seat(None, SystemState::Idle, false).nudgeable());
    assert!(seat(Some(SemanticState::Ready), SystemState::Idle, false).nudgeable());

    // Declared non-working states: deliberate silence, never a stall.
    //
    // `Waiting` belongs in THIS list, and an earlier draft of this table had it
    // in the one above — matching the implementation rather than the settled
    // guide ("eligible(seat) excludes waiting|hold|blocked|question"). The test
    // passed, which is the whole problem: it locked in the contradiction instead
    // of catching it. Caught by the cross-model reviewer, 2026-08-28.
    for parked in [
        SemanticState::Waiting,
        SemanticState::Hold,
        SemanticState::Blocked,
        SemanticState::Question,
        SemanticState::Done,
        SemanticState::Failed,
        SemanticState::Cancelled,
    ] {
        assert!(
            !seat(Some(parked), SystemState::Idle, false).nudgeable(),
            "{parked:?} is deliberate silence — nudging it is the parked-state bug"
        );
    }

    // A working seat is not stalled just because it is quiet: the long-tool-call
    // false-stall class.
    assert!(!seat(None, SystemState::Working, false).nudgeable());

    // A relay's idleness is its job, and a nudge into one becomes a real message
    // on somebody's phone.
    assert!(!seat(Some(SemanticState::Ready), SystemState::Idle, true).nudgeable());
}

#[test]
fn a_harness_that_is_not_recognised_is_reported_not_guessed() {
    assert_eq!(Harness::parse("claude"), Some(Harness::Claude));
    assert_eq!(Harness::parse("omp"), Some(Harness::Omp));
    assert_eq!(
        Harness::parse("pi "),
        None,
        "no trimming, no fuzzy matching: an unknown harness is a fact to report"
    );
    assert_eq!(
        Harness::parse("Claude"),
        None,
        "the wire spelling is lowercase"
    );
    assert_eq!(Harness::parse(""), None);

    // Round-trips, so the registry can store what it parsed.
    for harness in [
        Harness::Claude,
        Harness::Copilot,
        Harness::Codex,
        Harness::Pi,
        Harness::Omp,
    ] {
        assert_eq!(Harness::parse(harness.as_str()), Some(harness));
    }
}

#[test]
fn the_envelope_round_trips_and_omits_absent_data_rather_than_nulling_it() {
    let envelope = Envelope::ok("pij ping", "healthy".to_string());
    let json = serde_json::to_string(&envelope).expect("serialize");
    assert!(
        json.contains("\"v\":2"),
        "the envelope version travels: {json}"
    );
    assert_eq!(envelope.v, ENVELOPE_VERSION);

    let parsed: Envelope<String> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, envelope);

    // A failure carries no data at all — `data: null` would invite a reader to
    // treat absence as a value.
    let failed = Envelope::<String>::err("pij ping", "daemon is not running");
    let json = serde_json::to_string(&failed).expect("serialize");
    assert!(
        !json.contains("\"data\""),
        "an errored envelope omits `data` entirely: {json}"
    );
    assert!(json.contains("daemon is not running"));
}

#[test]
fn a_seat_descriptor_round_trips_with_its_optional_facts_intact() {
    let mut descriptor = SeatDescriptor::new("pij-seat", Harness::Copilot, "/abs/path");
    descriptor.parent = Some(SeatId::from("pij-prime"));
    descriptor.semantic_state = Some(SemanticState::Question);

    let json = serde_json::to_string(&descriptor).expect("serialize");
    let parsed: SeatDescriptor = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed, descriptor);
    assert_eq!(
        parsed.pane, None,
        "absent stays absent — it is not an empty string"
    );
    assert_eq!(
        parsed.parent.as_ref().map(SeatId::as_str),
        Some("pij-prime")
    );
}

#[test]
fn the_default_config_is_offline() {
    // Offline-first is a property of the DEFAULT, not a mode someone remembers
    // to select: a fresh checkout must run with no daemon, tmux, network or db.
    let config = Config::default();
    assert!(config.is_fully_offline());
    assert_eq!(config.adapters.registry, AdapterChoice::Fake);
    assert!(config.bind_addr.starts_with("127.0.0.1"), "loopback only");
    assert!(
        config.watchdog_interval_secs > 0,
        "the threshold is configured, never a hardcoded constant"
    );
    assert!(
        config.claim_lease_secs > 0,
        "claim expiry is configured, never embedded in queue logic"
    );
    assert_eq!(
        config.delivered_id_capacity, 1_024,
        "the per-recipient delivered-id bound is a chosen config policy"
    );
    assert!(
        config.event_buffer_capacity > 0,
        "the live stream is bounded but not disabled"
    );

    let mut real_store = config.clone();
    real_store.adapters.registry = AdapterChoice::Real;
    assert!(
        !real_store.is_fully_offline(),
        "one real adapter is enough to stop claiming offline"
    );
}

#[test]
fn errors_separate_the_states_they_observe_and_name_the_fix() {
    // TS's E-NOREG meant two different things with two different recoveries.
    let missing_row = PijError::NoRegistryEntry {
        seat: SeatId::from("pij-ghost"),
        store: "/Users/x/.pij-rs/pij.sqlite".to_string(),
    };
    let missing_artifact = PijError::NativeArtifactMissing {
        seat: SeatId::from("pij-ghost"),
        harness: "omp".to_string(),
        path: "/Users/x/.omp/sessions/abc.json".to_string(),
    };
    assert_ne!(missing_row, missing_artifact);

    let row = missing_row.to_string();
    assert!(
        row.contains("adopt"),
        "a recoverable error names the recovery: {row}"
    );
    // An absence is only evidence about the place that was SEARCHED, and the
    // cause is hedged rather than asserted. The sibling generation shipped an
    // E-NOREG blaming a missing extension when the real fact was HOME resolving
    // to a fixture path — the message diagnosed a cause nobody observed
    // (coral/meadowlark, cross-government; ruled by the prime for this wave).
    assert!(
        row.contains("/Users/x/.pij-rs/pij.sqlite"),
        "an absence names WHERE it looked: {row}"
    );
    assert!(
        row.contains("MAY"),
        "and HEDGES the cause it did not observe: {row}"
    );

    let artifact = missing_artifact.to_string();
    assert!(
        artifact.contains("cannot be revived"),
        "an unrecoverable one says so instead of inviting a retry: {artifact}"
    );
    assert!(
        artifact.contains("/Users/x/.omp/sessions/abc.json"),
        "and it names the artifact it went looking for: {artifact}"
    );

    // The 280-char limit that was undocumented in TS states itself.
    let too_long = PijError::ReportTooLong {
        len: 281,
        limit: 280,
    };
    assert!(too_long.to_string().contains("280"));
}

#[test]
fn a_delivery_claim_cannot_exist_without_saying_what_was_observed() {
    // erratum-23b, ruled by the prime as the v1 wire vocabulary. The defect this
    // closes was found by s105 in the TS tree: `confirmed` was returned
    // immediately after pressing Enter, having observed nothing at all. The TYPE
    // permitted the lie, so one got written.
    //
    // This test is a type-level assertion as much as a value one: `Delivered`
    // takes a required `origin`, so "confirmed, provenance unknown" cannot be
    // constructed at all.
    use pij_core::model::{DeliveryOrigin, DeliveryOutcome};

    let typed = DeliveryOutcome::Delivered {
        origin: DeliveryOrigin::TypedToPane,
    };
    let injected = DeliveryOutcome::Delivered {
        origin: DeliveryOrigin::InjectedToTransport,
    };
    let read = DeliveryOutcome::Delivered {
        origin: DeliveryOrigin::ReaderRead,
    };
    assert_ne!(
        typed, injected,
        "bytes typed into a pane and bytes accepted by a recipient process are different facts"
    );
    assert_ne!(
        injected, read,
        "bytes accepted by a socket and a recipient having read them are different facts"
    );

    // Strength order: pane typing is weaker than a recipient process accepting
    // socket bytes; explicit acknowledgements remain stronger than both.
    assert!(
        DeliveryOrigin::TypedToPane.strength() < DeliveryOrigin::InjectedToTransport.strength()
    );
    assert!(
        DeliveryOrigin::InjectedToTransport.strength() < DeliveryOrigin::VerifiedArrival.strength()
    );
    assert!(DeliveryOrigin::VerifiedArrival.strength() < DeliveryOrigin::ReaderRead.strength());

    // The governing rule: where observations differ, the WEAKER word is honest.
    assert_eq!(
        DeliveryOrigin::ReaderRead.weakest(DeliveryOrigin::InjectedToTransport),
        DeliveryOrigin::InjectedToTransport,
        "a consumer that cannot distinguish renders the weakest applicable claim"
    );
    assert_eq!(
        DeliveryOrigin::InjectedToTransport.weakest(DeliveryOrigin::VerifiedArrival),
        DeliveryOrigin::InjectedToTransport,
        "weakest is symmetric — it is about the evidence, not the argument order"
    );
    assert_eq!(
        DeliveryOrigin::ReaderRead.weakest(DeliveryOrigin::ReaderRead),
        DeliveryOrigin::ReaderRead
    );

    // The origin travels on the wire in the erratum's own words, so a TS consumer
    // and a Rust one read the same vocabulary.
    let json = serde_json::to_string(&injected).expect("serialize");
    assert!(
        json.contains("injected-to-transport"),
        "the wire word is the erratum's word, not a Rust-shaped rename: {json}"
    );
}
