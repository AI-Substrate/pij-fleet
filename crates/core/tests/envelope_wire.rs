use pij_core::model::{Envelope, ErrorKind};
use pij_testkit::fixtures;
use serde_json::Value;

#[test]
fn every_error_kind_emits_the_shared_complete_refusal_envelope() {
    let cases: Vec<Value> =
        serde_json::from_str(&fixtures::read("golden/api/error-envelopes.json"))
            .expect("shared ErrorKind refusal corpus");
    let mut seen = [false; 6];

    for expected in cases {
        let kind: ErrorKind = serde_json::from_value(expected["error"].clone())
            .expect("the shared vocabulary must decode as the native ErrorKind");
        // Deliberately exhaustive: adding a native variant requires reviewing the
        // shared corpus, not silently leaving the TypeScript decoder behind.
        let index = match kind {
            ErrorKind::Refused => 0,
            ErrorKind::NotFound => 1,
            ErrorKind::Auth => 2,
            ErrorKind::Skew => 3,
            ErrorKind::CursorReset => 4,
            ErrorKind::Adapter => 5,
        };
        assert!(!seen[index], "duplicate shared fixture for {kind:?}");
        seen[index] = true;

        let mut emitted = Envelope::<Value>::refused(
            expected["command"].as_str().expect("fixture command"),
            kind,
            expected["meta"].as_str().expect("fixture meta"),
        );
        emitted.data = expected.get("data").cloned();
        emitted.details = expected.get("details").cloned();
        assert_eq!(
            serde_json::to_value(&emitted).expect("serialize native refusal"),
            expected,
            "native {kind:?} emission must match the shared complete envelope"
        );
    }

    assert!(
        seen.into_iter().all(|covered| covered),
        "every native ErrorKind needs exactly one shared refusal fixture"
    );
}
