//! The wire: envelopes in, NDJSON out.
//!
//! Owned by ONE crate on purpose (workshop 001 R6d). In fs3's build the fence
//! rule — "two workers never touch the same file" — forced four units to
//! hand-roll ~90 lines of the same private helpers, and the four copies drifted.
//! Everything that turns a value into bytes on a socket lives here, so a later
//! unit imports it instead of re-deriving it.
//!
//! Two rules the whole port depends on:
//!
//! 1. **A reader refuses a future it cannot understand.** An envelope whose `v`
//!    exceeds this build's is rejected whole — half-reading a payload from a
//!    newer peer is worse than saying no.
//! 2. **A reader FORWARDS a fact it does not recognise.** An event of an unknown
//!    kind decodes to [`WireEvent::Unknown`], keeping its bytes. Dropping it
//!    would silently lose the newest information in the stream — which is the
//!    opposite failure from rule 1, and the distinction is deliberate: an
//!    envelope is a contract this build must satisfy, an event is a fact that
//!    happened whether or not this build has heard of it.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{ENVELOPE_VERSION, Envelope, Event};

/// Why a wire value could not be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The bytes are not JSON at all — a human-readable line printed into a
    /// machine stream, typically. Names the line so the source is findable.
    #[error("line {line}: not JSON ({detail}) — a machine stream carried human text")]
    NotJson {
        /// 1-based line number.
        line: usize,
        /// The parser's own words.
        detail: String,
    },
    /// The JSON parsed but is not the shape expected.
    #[error("line {line}: JSON is not a {expected} ({detail})")]
    WrongShape {
        /// 1-based line number.
        line: usize,
        /// What was expected.
        expected: String,
        /// The parser's own words.
        detail: String,
    },
    /// The envelope comes from a version this build does not understand.
    #[error(
        "envelope v{found} is newer than this build's v{expected} — upgrade pij rather than \
         acting on a payload whose meaning may have changed"
    )]
    FutureVersion {
        /// The version on the wire.
        found: u32,
        /// The version this build speaks.
        expected: u32,
    },
}

/// Encode an envelope as one JSON line, newline included.
///
/// # Errors
/// [`WireError::WrongShape`] when the payload itself cannot be serialised.
pub fn encode_envelope<T: Serialize>(envelope: &Envelope<T>) -> Result<String, WireError> {
    let json = serde_json::to_string(envelope).map_err(|error| WireError::WrongShape {
        line: 1,
        expected: "serialisable payload".to_string(),
        detail: error.to_string(),
    })?;
    Ok(format!("{json}\n"))
}

/// Decode an envelope, refusing anything newer than this build.
///
/// # Errors
/// [`WireError::NotJson`], [`WireError::WrongShape`] or
/// [`WireError::FutureVersion`].
pub fn decode_envelope<T: DeserializeOwned>(text: &str) -> Result<Envelope<T>, WireError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| WireError::NotJson {
            line: 1,
            detail: error.to_string(),
        })?;

    // The version is checked BEFORE the payload is typed: refusing on `v` must
    // not depend on this build being able to parse a shape it has never seen.
    let found = value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    if found > ENVELOPE_VERSION {
        return Err(WireError::FutureVersion {
            found,
            expected: ENVELOPE_VERSION,
        });
    }

    serde_json::from_value(value).map_err(|error| WireError::WrongShape {
        line: 1,
        expected: "envelope".to_string(),
        detail: error.to_string(),
    })
}

/// One line of an NDJSON event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireEvent {
    /// The stream's opening line, so a reader knows which daemon it reached and
    /// which version it speaks before any event arrives.
    Hello {
        /// The event-schema version the writer speaks.
        v: u32,
        /// The daemon's build id.
        build: String,
    },
    /// An event this build understands.
    Known(Event),
    /// An event this build has never heard of, KEPT rather than dropped: it is
    /// still a fact that happened, and the next build will know what it means.
    Unknown {
        /// The kind string, so a reader can at least count and forward it.
        kind: String,
        /// The original line, byte-for-byte.
        raw: String,
    },
}

/// Encode one event as an NDJSON line.
///
/// # Errors
/// [`WireError::WrongShape`] when the event cannot be serialised.
pub fn encode_event(event: &Event) -> Result<String, WireError> {
    let json = serde_json::to_string(event).map_err(|error| WireError::WrongShape {
        line: 1,
        expected: "event".to_string(),
        detail: error.to_string(),
    })?;
    Ok(format!("{json}\n"))
}

/// The stream's opening line.
///
/// # Errors
/// [`WireError::WrongShape`] when the hello cannot be serialised.
pub fn encode_hello(build: &str) -> Result<String, WireError> {
    let json = serde_json::to_string(&serde_json::json!({
        "hello": true,
        "v": EVENT_VERSION,
        "build": build,
    }))
    .map_err(|error| WireError::WrongShape {
        line: 1,
        expected: "hello".to_string(),
        detail: error.to_string(),
    })?;
    Ok(format!("{json}\n"))
}

/// The event-schema version this build writes.
pub const EVENT_VERSION: u32 = 1;

/// The kinds this build understands. An event whose kind is not here decodes to
/// [`WireEvent::Unknown`] — the list is what makes "unknown" a decision rather
/// than a parse accident.
pub const KNOWN_EVENT_KINDS: &[&str] = &[
    "seat.put",
    "seat.tombstone",
    "report",
    "receipt",
    "message",
    "job.enqueued",
    "job.acked",
    "delivery.held",
    "delivery.released",
    // Plan 158: held FYIs and a seat's own busy/idle publication.
    "fyi.held",
    "fyi.delivered",
    "seat.activity",
];

/// Decode one NDJSON line.
///
/// # Errors
/// [`WireError::NotJson`] or [`WireError::WrongShape`], each naming the line.
pub fn decode_event_line(line_number: usize, line: &str) -> Result<WireEvent, WireError> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|error| WireError::NotJson {
            line: line_number,
            detail: error.to_string(),
        })?;

    if value.get("hello").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(WireEvent::Hello {
            v: value
                .get("v")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32,
            build: value
                .get("build")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }

    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| WireError::WrongShape {
            line: line_number,
            expected: "event with a `kind`".to_string(),
            detail: "no `kind` field".to_string(),
        })?
        .to_string();

    if !KNOWN_EVENT_KINDS.contains(&kind.as_str()) {
        return Ok(WireEvent::Unknown {
            kind,
            raw: line.to_string(),
        });
    }

    serde_json::from_value(value)
        .map(WireEvent::Known)
        .map_err(|error| WireError::WrongShape {
            line: line_number,
            expected: "event".to_string(),
            detail: error.to_string(),
        })
}

/// Decode a whole NDJSON stream, one verdict per non-empty line.
///
/// Blank lines are skipped rather than reported: an NDJSON writer that flushes a
/// trailing newline is not malformed. Every other line yields a result, so ONE
/// bad line never costs the reader the rest of the stream — the property that
/// makes a stray warning printed into a machine stream survivable.
pub fn decode_event_stream(text: &str) -> Vec<Result<WireEvent, WireError>> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| decode_event_line(index + 1, line))
        .collect()
}
