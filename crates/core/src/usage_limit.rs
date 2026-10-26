//! Usage-limit resume (plan 172, ruling R3).
//!
//! When a fleet stops on a usage limit, nothing happens: no turns, no events,
//! so the PA watchdog's fleet digest never changes and nobody is told when the
//! limit lifts. This is the one case where "nothing changed" IS the problem.
//!
//! **Source: Claude Code's structured transcript entry, never its display
//! text.** A rejected call is recorded as an assistant entry with
//! `isApiErrorMessage: true`, `error: "rate_limit"`, `apiErrorStatus: 429` and
//! `quotaLimits: {status: "rejected", rateLimitType, resetsAt, …}`, where
//! `resetsAt` is an absolute epoch in seconds. The on-screen phrase ("resets
//! 3:10pm (Australia/Brisbane)") is never parsed. Other harnesses record no
//! equivalent structured field that pij knows of, so only Claude seats are read.
//!
//! **Time is epoch only.** `resetsAt` is stored and compared as UTC epoch
//! seconds; a wall-clock string exists only for display, rendered by the
//! composition edge in the local zone at that instant.

use serde_json::Value;

use crate::error::Result;
use crate::model::{Harness, SeatId};

/// How long after `resetsAt` the wake fires, so the provider has really
/// lifted the limit when the woken seat calls it.
pub const LIMIT_WAKE_MARGIN_SECS: u64 = 120;

/// The reason recorded on a limit-reset wake: it is exempt from the
/// cold-wake guard because the seat is cold by necessity.
pub const LIMIT_RESET_REASON: &str = "limit-reset";

/// A rejected call's structured quota facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRejection {
    /// `rateLimitType`, e.g. `five_hour` or `seven_day`.
    pub limit_type: String,
    /// `resetsAt`: when the limit lifts, in UTC epoch seconds.
    pub resets_at_secs: u64,
}

/// The quota rejection one transcript entry records, if it is one.
///
/// Every structured field must agree: an assistant API error of kind
/// `rate_limit`, status 429, whose `quotaLimits.status` is `rejected` with a
/// numeric `resetsAt`. Anything else is not a rejection.
pub fn rejection(record: &Value) -> Option<QuotaRejection> {
    if record.get("isApiErrorMessage") != Some(&Value::Bool(true))
        || record.get("error").and_then(Value::as_str) != Some("rate_limit")
        || record.get("apiErrorStatus").and_then(Value::as_u64) != Some(429)
    {
        return None;
    }
    let quota = record.get("quotaLimits")?;
    if quota.get("status").and_then(Value::as_str) != Some("rejected") {
        return None;
    }
    Some(QuotaRejection {
        limit_type: quota
            .get("rateLimitType")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        resets_at_secs: quota.get("resetsAt").and_then(Value::as_u64)?,
    })
}

/// The rejection a seat is stopped on right now, from its transcript's
/// newest lines (oldest first). Only the LAST assistant entry counts: a
/// rejection followed by a real call is over, and an old rejection deeper in
/// history never fires. Lines that are not JSON (a cut first line) are skipped.
pub fn latest_active<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<QuotaRejection> {
    let mut latest = None;
    for line in lines {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) == Some("assistant") {
            latest = rejection(&record);
        }
    }
    latest
}

/// When the one wake for `resets_at_secs` is due, in epoch ms. A reset found
/// after this instant is due at once.
pub const fn wake_due_ms(resets_at_secs: u64) -> u64 {
    resets_at_secs
        .saturating_add(LIMIT_WAKE_MARGIN_SECS)
        .saturating_mul(1_000)
}

/// Is a rejection still a stop at `now_ms`? Only until its wake is due
/// ([`wake_due_ms`]). After that the provider has lifted the limit, and a
/// seat whose last record is still the rejection is just a seat nobody
/// restarted (#68): it must not keep its scope "on limit".
pub const fn limit_current(resets_at_secs: u64, now_ms: u64) -> bool {
    now_ms < wake_due_ms(resets_at_secs)
}

/// Reads a seat's current quota rejection from its harness transcript.
///
/// Blocking file IO: the daemon calls it off the reactor. A harness without a
/// structured quota record answers `Ok(None)`, never a guess.
pub trait UsageLimitSource: Send + Sync {
    /// The rejection the seat's session is stopped on, if any.
    ///
    /// # Errors
    /// The transcript exists but could not be read.
    fn latest_rejection(&self, harness: Harness, session: &str) -> Result<Option<QuotaRejection>>;
}

/// The source for a daemon that reads no transcripts (fakes, offline).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoUsageLimits;

impl UsageLimitSource for NoUsageLimits {
    fn latest_rejection(&self, _: Harness, _: &str) -> Result<Option<QuotaRejection>> {
        Ok(None)
    }
}

/// Who receives a scope's limit-reset wake, and which record named them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WakeTarget {
    /// The PA in scope (its role, read from the role store).
    Pa(SeatId),
    /// No PA: the prime's current designation, when that seat is in scope.
    Designation(SeatId),
    /// No PA and no designation in scope: a live seat in scope holding the
    /// `prime` role.
    PrimeRole(SeatId),
}

impl WakeTarget {
    /// The seat.
    pub const fn seat(&self) -> &SeatId {
        match self {
            Self::Pa(seat) | Self::Designation(seat) | Self::PrimeRole(seat) => seat,
        }
    }

    /// The source, as recorded: `pa-role`, `prime-designation` or `prime-role`.
    pub const fn source(&self) -> &'static str {
        match self {
            Self::Pa(_) => "pa-role",
            Self::Designation(_) => "prime-designation",
            Self::PrimeRole(_) => "prime-role",
        }
    }
}

/// One fired limit-reset wake: at most one per `(scope, resets_at_secs)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitWake {
    /// The repository the stopped seats share (git common dir, or folder).
    pub scope: String,
    /// `resetsAt`, UTC epoch seconds.
    pub resets_at_secs: u64,
    /// `rateLimitType`.
    pub limit_type: String,
    /// Who was woken; `None` when the scope had no PA and no prime.
    pub target: Option<SeatId>,
    /// [`WakeTarget::source`], or `none`.
    pub target_source: String,
    /// When it fired, epoch ms.
    pub fired_at_ms: u64,
}

/// The limit-reset wake's text. `local` is `resets_at_secs` rendered in the
/// local zone, with the zone's name.
pub fn wake_body(
    limit_type: &str,
    resets_at_secs: u64,
    local: &str,
    stopped: &[SeatId],
    late: bool,
) -> String {
    let seats = stopped
        .iter()
        .map(SeatId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let when = if late {
        format!("reset at {local} (epoch {resets_at_secs}); seen only now")
    } else {
        format!("reset at {local} (epoch {resets_at_secs})")
    };
    format!(
        "[pij watchdog] usage limit lifted: the {limit_type} limit that stopped {seats} {when}. \
         Decide which seats to restart and send them their resume; the watchdog wakes nobody else. \
         This wake is exempt from the cold-wake guard ({LIMIT_RESET_REASON})."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape verified 2026-10-13 against Claude Code's on-screen text.
    fn rejected(resets_at: u64, kind: &str) -> String {
        format!(
            r#"{{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","apiErrorStatus":429,"quotaLimits":{{"status":"rejected","rateLimitType":"{kind}","resetsAt":{resets_at},"isUsingOverage":false}},"message":{{"model":"<synthetic>","content":[{{"type":"text","text":"You've hit your limit · resets 3:10pm (Australia/Brisbane)"}}]}}}}"#
        )
    }

    const CALL: &str =
        r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1}}}"#;

    #[test]
    fn only_every_structured_field_together_is_a_rejection() {
        let line = rejected(1_790_399_400, "five_hour");
        assert_eq!(
            latest_active([line.as_str()]),
            Some(QuotaRejection {
                limit_type: "five_hour".into(),
                resets_at_secs: 1_790_399_400,
            })
        );
        for broken in [
            line.replace(r#""status":"rejected""#, r#""status":"allowed_warning""#),
            line.replace(r#""apiErrorStatus":429"#, r#""apiErrorStatus":529"#),
            line.replace(r#""error":"rate_limit""#, r#""error":"overloaded""#),
            line.replace(
                r#""isApiErrorMessage":true"#,
                r#""isApiErrorMessage":false"#,
            ),
            line.replace(r#""resetsAt":1790399400"#, r#""resetsAt":"3:10pm""#),
        ] {
            assert_eq!(latest_active([broken.as_str()]), None, "{broken}");
        }
    }

    #[test]
    fn a_call_after_the_rejection_ends_it_and_cut_lines_are_skipped() {
        let line = rejected(10, "seven_day");
        assert_eq!(latest_active([line.as_str(), CALL]), None);
        let user = r#"{"type":"user","message":{"content":"retry"}}"#;
        assert!(latest_active(["ected\"}}", CALL, line.as_str(), user]).is_some());
    }

    #[test]
    fn the_wake_is_due_a_margin_after_the_reset() {
        assert_eq!(wake_due_ms(1_000), (1_000 + LIMIT_WAKE_MARGIN_SECS) * 1_000);
    }

    #[test]
    fn a_limit_stops_until_its_wake_is_due_and_no_longer() {
        let due = wake_due_ms(1_000);
        assert!(limit_current(1_000, 0));
        assert!(
            limit_current(1_000, 1_000 * 1_000),
            "the margin still stops"
        );
        assert!(limit_current(1_000, due - 1));
        assert!(!limit_current(1_000, due), "lifted once the wake is due");
    }
}
