//! The fixture corpus is addressable, intact, and carries its answers (R6b).
//!
//! The corpus is only worth shipping if a packet can cite a path and get the
//! same bytes every time. These tests are what makes that true: the manifest and
//! the tree must agree in BOTH directions, and the checksum they agree on has to
//! be a real sha256 rather than a function that agrees with itself.

use pij_testkit::fixtures;

#[test]
fn the_manifest_and_the_corpus_agree_both_ways() {
    let faults = fixtures::verify_manifest();
    assert!(
        faults.is_empty(),
        "corpus and MANIFEST.toml disagree:\n{}",
        faults
            .iter()
            .map(|fault| format!("  - {fault}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_fixture_states_what_a_correct_implementation_must_do() {
    // A fixture without its expectation is just a file: the next agent has bytes
    // and no way to know what they prove.
    for fixture in fixtures::manifest().fixtures {
        assert!(
            fixture.answer.len() > 20,
            "{}: `answer` must say what a correct implementation must DO with these bytes",
            fixture.path
        );
        assert!(
            !fixture.note.is_empty(),
            "{}: `note` must say where the bytes came from",
            fixture.path
        );
    }
}

#[test]
fn the_corpus_includes_deliberately_broken_cases() {
    // Sailfish's sharpening of testkit-first: known-GOOD fixtures only prove the
    // happy path, and every parser bug this port will hit lives in the other set.
    let malformed: Vec<String> = fixtures::index()
        .into_keys()
        .filter(|path| path.starts_with("malformed/"))
        .collect();
    assert!(
        malformed.len() >= 5,
        "the corpus must carry deliberately-broken cases, found {malformed:?}"
    );

    // The two that decide real behaviour later, named explicitly so a rename
    // cannot quietly drop them.
    assert!(malformed.contains(&"malformed/rs/unknown-event-kind.ndjson".to_string()));
    assert!(malformed.contains(&"malformed/rs/future-envelope.json".to_string()));

    // Both families must exist: the v1 decoder is judged against rs/, and the
    // wave-3 migration reader against captured ts/ bytes. Collapsing them would
    // hide that the two schemas genuinely differ.
    assert!(
        malformed
            .iter()
            .any(|path| path.starts_with("malformed/rs/"))
    );
    assert!(
        malformed
            .iter()
            .any(|path| path.starts_with("malformed/ts/"))
    );
}

#[test]
fn the_captured_cli_fixtures_are_the_real_shapes_not_hand_written_ones() {
    // Captured from the live TS pij, so the port is tested against what the
    // product actually emits rather than what someone remembered it emitting.
    let models = fixtures::read("cli/models.json");
    assert!(
        models.contains("\"requestModelId\"") && models.contains("\"selector\""),
        "the model catalog fixture must carry the s106 runtime/provider/selector shape"
    );
    // Two encodings of "this model has no thinking levels" appear in real
    // output — `"levels": []` and the key ABSENT — and NEITHER is the `null`
    // that services.dd.json assumes. Both are in the corpus so u-models cannot
    // be written against a shape the product does not emit.
    assert!(
        models.contains("\"levels\": []"),
        "the corpus must carry the empty-list encoding of 'no levels'"
    );
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&models).expect("the models fixture is a JSON array");
    assert!(
        rows.iter().any(|row| row.get("levels").is_none()),
        "the corpus must also carry the absent-key encoding — a reader that only \
         handles [] will silently misread every claude row"
    );
    assert!(
        !models.contains("\"levels\": null"),
        "no captured row uses null; if one ever does, re-record the manifest answer \
         rather than letting three encodings accumulate unnoticed"
    );

    let whoami = fixtures::read("cli/whoami.json");
    assert!(
        whoami.contains("\"folder\"") && whoami.contains("\"dataDir\""),
        "whoami must carry both the working folder and the data dir"
    );

    let events = fixtures::read("wire/events.ndjson");
    assert!(
        events.lines().count() >= 4,
        "the wire corpus needs enough events to exercise a tail"
    );
    for (index, line) in events.lines().enumerate() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "wire/events.ndjson line {} must be one complete JSON object",
            index + 1
        );
    }
}

#[test]
fn sha256_matches_the_published_vectors() {
    // The corpus check is only worth having if its checksum is a real sha256 and
    // not a function that merely agrees with itself. NIST's vectors decide it.
    assert_eq!(
        fixtures::sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        fixtures::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        fixtures::sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    // Crosses the 64-byte block boundary, which is where a hand-rolled padding
    // bug hides.
    assert_eq!(
        fixtures::sha256_hex(&b"a".repeat(1000)),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}

#[test]
fn a_changed_fixture_is_reported_against_its_recorded_answer() {
    // The failure mode worth catching: someone edits a fixture, the recorded
    // `answer` silently stops describing it, and every test that cites it is now
    // asserting something nobody chose.
    let fault = pij_testkit::fixtures::CorpusFault::ContentChanged {
        path: "cli/models.json".to_string(),
        expected: "aaa".to_string(),
        found: "bbb".to_string(),
    };
    let rendered = fault.to_string();
    assert!(
        rendered.contains("re-read the `answer`"),
        "the diagnostic must send the reader to the expectation, not only the hash: {rendered}"
    );
}
