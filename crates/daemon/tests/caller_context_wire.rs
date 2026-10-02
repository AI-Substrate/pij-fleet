//! THE CROSS-RUNTIME WIRE CONTRACT for `caller` — the Rust half.
//!
//! # What this replaces, and why
//!
//! The generation shim (TypeScript) and this daemon (Rust) never agreed on how
//! to spell the caller block. TS sent `pijSessionId`; Rust accepted
//! `PIJ_SESSION_ID`. Six of nine keys never met, the block deserialized to
//! ALL-NONE, and `whoami`/`phonehome` could not name a seat. **Both suites were
//! green the entire time**, because each side asserted its own spelling.
//!
//! The root cause was not a rename. The wire contract was distributed as PROSE —
//! a flat list of names in a message — while the TS source held PAIRS of
//! (env name, WIRE name). Each reader's interpretation was internally
//! consistent, so nothing could see the disagreement. A contract that exists as
//! prose has one definition per reader.
//!
//! # Why a fixture and not a scanner
//!
//! The first fix attempted was a scan of Rust source for the TS key literals.
//! It could not work, and its failure is instructive: a scanner must model serde
//! to be right, and to catch THIS bug it must model that **serde silently drops
//! unknown fields** — the very mechanism that made the defect invisible. A
//! half-renamed key was injected into the reader and the scan still passed.
//!
//! So the contract is a FILE that both runtimes consume, and each side asserts
//! on POPULATED VALUES rather than on the absence of a mismatch. That is the
//! property the scanner lacked: neither half can pass vacuously.
//!
//!   - TS half: `buildCallerContext(...)` deep-equals this fixture.
//!   - Rust half (here): the fixture deserializes with EVERY field present, and
//!     every key in it is CONSUMED.
//!
//! Rename a key on either side and that side goes red. There is no spelling
//! premise left for anyone to get wrong.

use pij_daemon::http::CallerContext;

/// The one artifact both runtimes read. Not a copy of the wire shape — THE wire
/// shape.
const FIXTURE: &str = include_str!("fixtures/caller-context.wire.json");

/// Every field the reader can populate. Kept as a list rather than a count so a
/// failure names the field, and so adding a field to `CallerContext` without
/// adding it to the fixture is a compile-adjacent, readable break.
const FIELDS: [&str; 9] = [
    "pijSessionId",
    "tmuxPane",
    "pijParentId",
    "claudeCodeSessionId",
    "copilotAgentSessionId",
    "codexThreadId",
    "cwd",
    "pid",
    "procStart",
];

/// THE LOAD-BEARING ASSERTION: every field arrives POPULATED.
///
/// Asserting on populated values is the whole design. A test that checked "no
/// unknown spelling appears" would pass on a struct that accepts keys nothing
/// sends — which is exactly the state that shipped. `None` here means the key
/// crossed the wire and the reader threw it away.
#[test]
fn every_field_of_the_wire_contract_is_read() {
    let caller: CallerContext = serde_json::from_str(FIXTURE).expect("the fixture is valid JSON");

    let missing: Vec<&str> = [
        ("pijSessionId", caller.session_id.is_some()),
        ("tmuxPane", caller.pane.is_some()),
        ("pijParentId", caller.parent.is_some()),
        ("claudeCodeSessionId", caller.claude_session.is_some()),
        ("copilotAgentSessionId", caller.copilot_session.is_some()),
        ("codexThreadId", caller.codex_session.is_some()),
        ("cwd", caller.cwd.is_some()),
        ("pid", caller.pid.is_some()),
        ("procStart", caller.proc_start.is_some()),
    ]
    .into_iter()
    .filter_map(|(key, read)| (!read).then_some(key))
    .collect();

    assert!(
        missing.is_empty(),
        "the shim SENDS these keys and this reader DROPPED them: {missing:?}\n\
         serde ignores what it does not recognise, so this is silent in production: the caller \
         block arrives, deserializes, and names no seat. Add the wire spelling to \
         `CallerContext` in crates/daemon/src/http/identity.rs."
    );
}

/// The other direction, and it closes the hole the first assertion leaves open.
///
/// "Every field is Some" would still pass if the fixture carried a TENTH key
/// this reader has never heard of — serde would drop it in silence, which is the
/// original defect wearing the fixture's clothes. Comparing the key SET catches
/// a key the shim sends and Rust does not model, and a key Rust models that the
/// shim never sends.
#[test]
fn the_fixture_and_the_reader_describe_the_same_key_set() {
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(FIXTURE).expect("the fixture is a JSON object");
    let mut in_fixture: Vec<&str> = object.keys().map(String::as_str).collect();
    let mut modelled = FIELDS.to_vec();
    in_fixture.sort_unstable();
    modelled.sort_unstable();

    assert_eq!(
        in_fixture, modelled,
        "the fixture and the reader disagree about WHICH keys exist. A key present only in the \
         fixture is one this daemon will silently drop; a key present only here is one the shim \
         never sends, so `every_field_of_the_wire_contract_is_read` would fail for a reason that \
         is not the reader's fault."
    );
}

/// Types are part of the contract too. `pid` and `procStart` cross as JSON
/// NUMBERS, not strings — a shim that quoted them would deserialize to `None`
/// for those two fields alone, which reads as "the caller had no process" and is
/// indistinguishable from a genuinely paneless caller.
#[test]
fn the_numeric_fields_cross_as_numbers() {
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(FIXTURE).expect("the fixture is a JSON object");
    for key in ["pid", "procStart"] {
        assert!(
            object[key].is_number(),
            "`{key}` must cross as a number: quoted, it deserializes to None and the seat reads \
             as having no process at all"
        );
    }
}
