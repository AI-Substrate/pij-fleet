//! The error taxonomy.
//!
//! Two rules the TS CLI taught us, both encoded here rather than remembered:
//!
//! 1. **An instrument must separate the states it observes.** `E-NOREG` meant
//!    both "no registry row" and "the native session artifact is gone" — the same
//!    code for two situations with DIFFERENT recoveries, so `revive` could not
//!    tell a repairable seat from an unrecoverable one. They are separate
//!    variants here, permanently.
//! 2. **An error names its own fix.** A refusal that does not say what to do
//!    costs a round trip to interpret, and agents pay that round trip in tokens.

use crate::model::SeatId;

/// Everything this workspace refuses to do, and why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PijError {
    /// A subscription asked for a cursor this spine has never reached.
    ///
    /// Not a transient: it describes a history that does not exist, which happens
    /// when a daemon is reprovisioned or its store wiped while its alias
    /// survives. Named rather than filtered, because silently returning nothing
    /// for a cursor that can never be satisfied leaves a consumer waiting for
    /// ever on a stream that reads healthy (review F7, wave 5).
    #[error(
        "requested cursor {requested} is beyond this spine's newest {newest} — that history MAY have been reset (reprovision, wiped store, or a cursor from a different machine); resume from the source's real position"
    )]
    CursorBeyondSpine {
        /// What the consumer asked for.
        requested: u64,
        /// The newest sequence this spine actually holds.
        newest: u64,
    },
    /// No registry row for this seat. Recoverable: adopt or register it.
    #[error(
        "{seat}: no registry row in {store} — the seat MAY never have registered, or this \
         daemon may be reading a different store than the one you expect; adopt it \
         (`pij adopt`) or spawn it again"
    )]
    NoRegistryEntry {
        /// WHERE we actually looked. Printed because an absence is only evidence
        /// about the place that was searched, and the sibling generation shipped
        /// `E-NOREG` asserting "is the extension loaded?" when the real fact was
        /// `HOME` resolving `~/.pij` to a fixture path (coral/meadowlark,
        /// cross-government). **An absence observed is not a cause diagnosed.**
        store: String,
        /// The seat that has no row.
        seat: SeatId,
    },

    /// The registry row exists, but the harness's own session artifact — the
    /// thing a revive replays — is gone. NOT recoverable by re-registering, and
    /// the reason this is a separate variant from [`PijError::NoRegistryEntry`].
    #[error(
        "{seat}: registry row present but the {harness} session artifact is missing at {path} \
         — this seat cannot be revived; close it and spawn a fresh one"
    )]
    NativeArtifactMissing {
        /// The seat whose artifact is gone.
        seat: SeatId,
        /// Which harness owned the artifact.
        harness: String,
        /// Where it was expected.
        path: String,
    },

    /// The store's schema is older or newer than this binary expects.
    ///
    /// Never limps: a half-understood schema corrupts quietly, and the skew
    /// direction decides the fix, so the message carries both versions.
    #[error("store schema is v{found} but this binary speaks v{expected} — {fix}")]
    StoreSchemaStale {
        /// What the store reports.
        found: u32,
        /// What this binary needs.
        expected: u32,
        /// The direction-specific fix.
        fix: String,
    },

    /// A manual pull must not compete with an attested native receiver.
    #[error(
        "{seat}: native receiver lease is live for another {expires_in_ms} ms — retry manual inbox after it expires"
    )]
    NativeReceiverLive {
        /// The inbox whose receiver still owns delivery.
        seat: SeatId,
        /// Remaining receiver lease, rounded up to milliseconds.
        expires_in_ms: u64,
    },

    /// A seat exists but cannot currently receive.
    #[error("{seat}: not deliverable ({reason}) — the message was queued, not lost")]
    NotDeliverable {
        /// The seat.
        seat: SeatId,
        /// Why not.
        reason: String,
    },

    /// The registry row is a post-mortem; this seat will never receive again.
    #[error(
        "{seat}: seat is tombstoned ({reason}) — the message was not queued because a tombstoned seat will never receive it",
        reason = tombstone_reason.as_deref().unwrap_or("no reason recorded")
    )]
    SeatIsGone {
        /// The seat whose row is tombstoned.
        seat: SeatId,
        /// The reason retained in the post-mortem row, when present.
        tombstone_reason: Option<String>,
    },

    /// Delivery acted, but the separate durable audit write failed.
    #[error(
        "message {msg_id} was {outcome}, but its audit event was not written ({audit_error}) — reconcile by msg-id; do not blindly resend"
    )]
    DeliveryAuditFailed {
        /// The message whose action already landed.
        msg_id: String,
        /// The action that landed: queued, delivered, or held.
        outcome: String,
        /// The event bus failure, preserved verbatim.
        audit_error: String,
    },

    /// A report card exceeded the documented limit.
    ///
    /// The limit is a const with a name, and the error states it: TS defect #7
    /// was an undocumented 280-char truncation that silently ate the end of a
    /// card.
    #[error("report card is {len} characters; the limit is {limit} — shorten it")]
    ReportTooLong {
        /// Actual length after collapsing whitespace.
        len: usize,
        /// The limit, from `report::CARD_LIMIT`.
        limit: usize,
    },

    /// A seat tried to address itself.
    #[error("{seat}: a seat cannot send to itself — use `pij compact-self` for self-compaction")]
    SelfAddressed {
        /// The seat.
        seat: SeatId,
    },

    /// A governance transaction refused before committing any row or event.
    #[error("{code}: {record}")]
    GovernanceRefused {
        /// Stable machine-readable refusal code, not parsed from display text.
        code: String,
        /// Record identity observed at refusal, such as the current lease id.
        record: String,
    },

    /// An adapter failed at its boundary. Carries the adapter's own words: a
    /// rewritten cause is a lost cause.
    #[error("{adapter}: {message}")]
    Adapter {
        /// Which adapter.
        adapter: String,
        /// What it said, verbatim.
        message: String,
    },
}

/// The workspace's result type.
///
/// The error parameter is defaulted rather than fixed, so the common call site
/// stays `Result<T>` while a module needing a narrower error can still name one.
pub type Result<T, E = PijError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::PijError;
    use crate::model::SeatId;

    #[test]
    fn tombstoned_refusal_says_why_nothing_was_queued() {
        let error = PijError::SeatIsGone {
            seat: SeatId::from("pij-gone"),
            tombstone_reason: Some("process exited".to_string()),
        }
        .to_string();

        assert!(error.contains("pij-gone: seat is tombstoned"));
        assert!(error.contains("process exited"));
        assert!(error.contains("message was not queued"));
        assert!(error.contains("will never receive it"));
    }

    #[test]
    fn audit_failure_names_the_effect_that_landed_and_forbids_blind_resend() {
        let error = PijError::DeliveryAuditFailed {
            msg_id: "m-42".to_string(),
            outcome: "queued".to_string(),
            audit_error: "spine unavailable".to_string(),
        }
        .to_string();

        assert!(error.contains("message m-42 was queued"));
        assert!(error.contains("audit event was not written"));
        assert!(error.contains("reconcile by msg-id"));
        assert!(error.contains("do not blindly resend"));
    }
}
