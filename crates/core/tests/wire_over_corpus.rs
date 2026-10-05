//! The wire, judged against the CORPUS — including its deliberately broken half.
//!
//! Every assertion here cites a fixture by path, which is the point of shipping
//! an addressable corpus: the malformed cases were captured from the real product
//! (or built to model a real incident), so this decoder is written against what
//! actually arrives rather than what a happy-path test imagines.

use pij_core::model::{Envelope, Event};
use pij_core::wire::{self, WireError, WireEvent};
use pij_testkit::fixtures;

#[test]
fn a_stray_human_line_costs_that_line_and_nothing_else() {
    // `malformed/rs/not-json.ndjson` — this exact class ("Warning: Set a purpose
    // first.") printed into a machine stream bricked a pi boot. One bad line must
    // never cost the reader the rest of the stream.
    let results = wire::decode_event_stream(&fixtures::read("malformed/rs/not-json.ndjson"));

    assert_eq!(results.len(), 2, "both lines get a verdict");
    match &results[0] {
        Err(WireError::NotJson { line, .. }) => assert_eq!(*line, 1, "the failure names its line"),
        other => panic!("the warning line must be reported as not-JSON: {other:?}"),
    }
    assert!(
        results[1].is_ok(),
        "the VALID line after it must still arrive: {:?}",
        results[1]
    );
}

#[test]
fn an_unknown_event_kind_is_forwarded_not_dropped() {
    // `malformed/rs/unknown-event-kind.ndjson`. The asymmetry that matters: an
    // envelope from the future is REFUSED, an event from the future is KEPT. An
    // event is a fact that happened whether or not this build knows the word for
    // it, and dropping it silently loses the newest information in the stream.
    let results =
        wire::decode_event_stream(&fixtures::read("malformed/rs/unknown-event-kind.ndjson"));

    assert_eq!(results.len(), 2);
    match results[0]
        .as_ref()
        .expect("an unknown kind is not an error")
    {
        WireEvent::Unknown { kind, raw } => {
            assert_eq!(kind, "quantum_entanglement");
            assert!(
                raw.contains("quantum_entanglement"),
                "the original bytes are kept so a newer build can still read them"
            );
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
    assert!(results[1].is_ok(), "the known line still decodes");
}

#[test]
fn a_truncated_line_is_reported_with_its_line_number() {
    let results = wire::decode_event_stream(&fixtures::read("malformed/rs/truncated-event.ndjson"));
    assert_eq!(results.len(), 1);
    assert!(matches!(
        results[0],
        Err(WireError::NotJson { line: 1, .. })
    ));
}

#[test]
fn an_empty_stream_is_empty_not_broken() {
    assert!(
        wire::decode_event_stream(&fixtures::read("malformed/rs/empty.ndjson")).is_empty(),
        "zero bytes is a valid, empty stream"
    );
    // A trailing newline is how every well-behaved NDJSON writer ends; treating
    // it as a malformed line would fail on correct input.
    assert_eq!(
        wire::decode_event_stream(
            "{\"kind\":\"receipt\",\"v\":1,\"at\":0,\"seat\":null,\"payload\":\"{}\"}\n\n"
        )
        .len(),
        1
    );
}

#[test]
fn an_envelope_from_the_future_is_refused_whole() {
    // `malformed/rs/future-envelope.json` — v=99. Refused BEFORE the payload is
    // typed, because declining must not depend on being able to parse a shape
    // this build has never seen.
    let error = wire::decode_envelope::<serde_json::Value>(&fixtures::read(
        "malformed/rs/future-envelope.json",
    ))
    .expect_err("a newer envelope must be refused");

    match error {
        WireError::FutureVersion { found, expected } => {
            assert_eq!((found, expected), (99, pij_core::model::ENVELOPE_VERSION));
        }
        other => panic!("wrong error: {other:?}"),
    }
    assert!(
        error.to_string().contains("upgrade pij"),
        "the refusal names the fix: {error}"
    );
}

#[test]
fn the_hello_line_comes_first_and_declares_the_version() {
    let hello = wire::encode_hello("pij-rs test").expect("encode");
    assert!(hello.ends_with('\n'), "NDJSON lines are newline-terminated");

    match wire::decode_event_line(1, hello.trim()).expect("decode") {
        WireEvent::Hello { v, build } => {
            assert_eq!(v, wire::EVENT_VERSION);
            assert_eq!(build, "pij-rs test");
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn a_known_event_round_trips_through_the_wire() {
    let event = Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: 1_724_800_000_000,
        kind: "report".to_string(),
        seat: Some("pij-seat".into()),
        payload: "{\"did\":\"a thing\"}".to_string(),
    };

    let line = wire::encode_event(&event).expect("encode");
    match wire::decode_event_line(1, line.trim()).expect("decode") {
        WireEvent::Known(decoded) => assert_eq!(decoded, event),
        other => panic!("expected Known, got {other:?}"),
    }
}

/// Plan 158: the facts a held FYI and a seat's own busy/idle publication
/// produce are this build's own kinds, decoded as known, not forwarded as unknown.
#[test]
fn plan_158_event_kinds_are_known() {
    let events = [
        pij_core::fyi::held_event(&pij_core::fyi::HeldFyi {
            id: "m-1".to_string(),
            recipient: "pij-seat".into(),
            sender: "pij-sender".into(),
            from_machine: None,
            body: "note".to_string(),
            held_at_ms: 1,
        }),
        pij_core::fyi::delivered_event(&"pij-seat".into(), &["m-1".to_string()], "hook:omp", 2),
        Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: 3,
            kind: "seat.activity".to_string(),
            seat: Some("pij-seat".into()),
            payload: "{\"state\":\"working\"}".to_string(),
        },
    ];
    for event in events {
        let line = wire::encode_event(&event).expect("encode");
        match wire::decode_event_line(1, line.trim()).expect("decode") {
            WireEvent::Known(decoded) => assert_eq!(decoded, event),
            other => panic!("{}: expected Known, got {other:?}", event.kind),
        }
    }
}

#[test]
fn an_envelope_round_trips_and_a_current_version_is_accepted() {
    let envelope = Envelope::ok("pij ping", serde_json::json!({"status": "healthy"}));
    let text = wire::encode_envelope(&envelope).expect("encode");
    let decoded: Envelope<serde_json::Value> =
        wire::decode_envelope(&text).expect("a same-version envelope decodes");
    assert_eq!(decoded, envelope);
}

#[test]
fn an_event_without_a_kind_is_a_shape_error_not_an_unknown_kind() {
    // "I do not recognise this kind" and "this is not an event" are different
    // facts with different recoveries, so they are different variants.
    let line = fixtures::read("malformed/rs/no-kind.ndjson");
    let error =
        wire::decode_event_line(7, line.trim()).expect_err("an event with no kind is malformed");
    match error {
        WireError::WrongShape { line, .. } => assert_eq!(line, 7),
        other => panic!("wrong error: {other:?}"),
    }
}

#[test]
fn a_sequenced_event_never_carries_its_cursor_onto_the_wire() {
    // R7 fixes the v1 event shape at exactly {v, at, kind, seat, payload}. The
    // store assigns `seq` on the read path so a consumer can advance a durable
    // cursor, but that is a property of the STREAM, not of the fact that
    // happened — an event that has been tailed and one that has just been
    // published must look identical on the wire.
    //
    // This test exists because the first version of the seam shipped
    // `skip_serializing_if = "Option::is_none"`, which serialises `seq` the
    // moment it is `Some` — the doc comment said one thing and the attribute did
    // another. Caught by a coder reading the merged code rather than the receipt.
    let tailed = Event {
        seq: Some(pij_core::model::Seq(42)),
        v: wire::EVENT_VERSION,
        at: 1_724_800_000_000,
        kind: "report".to_string(),
        seat: Some("pij-seat".into()),
        payload: "{}".to_string(),
    };

    let line = wire::encode_event(&tailed).expect("encode");
    assert!(
        !line.contains("seq") && !line.contains("42"),
        "the cursor must not reach the wire: {line}"
    );

    let published = Event {
        seq: None,
        ..tailed.clone()
    };
    assert_eq!(
        wire::encode_event(&published).expect("encode"),
        line,
        "a tailed event and a freshly published one must be byte-identical on the wire"
    );

    // ...and decoding never invents one.
    match wire::decode_event_line(1, line.trim()).expect("decode") {
        WireEvent::Known(decoded) => assert_eq!(decoded.seq, None),
        other => panic!("expected Known, got {other:?}"),
    }
}
