//! Daemon notice message ids (F141).
//!
//! A daemon notice's id is its dedupe key, and most are predictable (built from
//! a seat, a request id, a time). A caller who could submit a message under the
//! id a notice will use would take its slot, and the notice would then be
//! deduplicated away or refused. So every daemon notice id starts with a prefix
//! from this table, and external admission refuses any caller id that does
//! ([`reserved`]): no caller can write in the daemon's namespace.
//!
//! Every daemon id is minted here ([`NoticeKind::id`]); a source scan
//! (`crates/daemon/tests/notice_ids.rs`) fails the build on a daemon id built
//! any other way.

use std::fmt::Display;

/// The refusal code for a caller id inside a daemon namespace.
pub const RESERVED_CODE: &str = "E-RS-MSG-ID-RESERVED";

/// Declares every kind, its doc and its prefix once, and generates the enum,
/// [`NoticeKind::ALL`] and [`NoticeKind::prefix`] from that one list: a kind
/// can't exist without a prefix, or be missing from `ALL`.
macro_rules! notice_kinds {
    ($( $(#[$doc:meta])* $kind:ident => $prefix:literal, )+) => {
        /// One kind of daemon-authored message, by the id prefix it owns.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum NoticeKind {
            $( $(#[$doc])* $kind, )+
        }

        impl NoticeKind {
            /// Every kind, so the reserved table can't miss one.
            pub const ALL: &'static [Self] = &[ $( Self::$kind, )+ ];

            /// The id prefix this kind owns. These bytes are the dedupe keys of
            /// rows already in live stores: change one and its notices dedupe
            /// afresh.
            #[must_use]
            pub const fn prefix(self) -> &'static str {
                match self {
                    $( Self::$kind => $prefix, )+
                }
            }
        }
    };
}

notice_kinds! {
    /// A background job's batches, final turn and cold notices.
    PijBg => "pij-bg:",
    /// A sidecar background job was cancelled or ended (`pij sidecar bg`).
    SidecarBg => "bg-",
    /// A message parked after its delivery attempts ran out.
    ParkNotice => "park-notice-",
    /// A message held at the hold limit.
    HoldNotice => "hold-notice-",
    /// The prime has no live PA.
    PaMissing => "pa-missing:",
    /// The PA is dormant.
    PaDormant => "pa-dormant:",
    /// The PA does not answer.
    PaUnresponsive => "pa-unresponsive:",
    /// Seats over the size limit, told to the prime.
    LargeSeats => "large-seats:",
    /// A PA watchdog round.
    PaWatchdog => "pa-watchdog:",
    /// The prime is active, told to the PA.
    PrimeActive => "prime-active:",
    /// A usage limit has reset.
    LimitReset => "limit-reset:",
    /// A steer control was held.
    SteerControl => "steer-control:",
    /// A watchdog control was held.
    WatchdogControl => "watchdog-control:",
    /// The routing table, after a compact.
    RoutesAfterCompact => "routes-after-compact:",
    /// A role request was closed.
    RequestClosed => "request-closed-",
    /// A role request was withdrawn (its role retired or merged).
    RequestWithdrawn => "request-withdrawn-",
    /// The memory turn before a compact or close.
    MemoryTurn => "memory-turn-",
    /// A seat a compaction left idle is woken.
    CompactionWake => "compaction-wake:",
    /// Its parent is told about that wake.
    CompactionWakeFyi => "compaction-wake-fyi:",
    /// A role request queued for a busy role.
    RoleQueue => "role-queue-",
    /// A role request's time-limit check-in.
    RoleLimitCheckIn => "role-limit-checkin-",
    /// A role request's time-limit notice.
    RoleLimitNotice => "role-limit-notice-",
    /// Held FYIs flushed into one turn.
    FyiFlush => "fyi-flush-",
    /// A seat idle while holding work.
    IdleNudge => "idle-nudge:",
    /// Its prime is told about that nudge.
    IdleNudgeFyi => "idle-nudge-fyi:",
    /// A seat stalled on a modal.
    ModalStall => "modal-stall:",
    /// An output was written.
    Output => "output-",
    /// A seat running twice.
    DoubleSeat => "double-seat-",
    /// A chore was defined or ran.
    Chore => "chore-",
    /// An inbound Telegram message.
    Telegram => "telegram-",
}

impl NoticeKind {
    /// This kind's id for one occurrence: its prefix, then `rest`.
    #[must_use]
    pub fn id(self, rest: impl Display) -> String {
        format!("{}{rest}", self.prefix())
    }
}

/// The daemon kind whose namespace `msg_id` is in, if any. External admission
/// refuses such an id ([`RESERVED_CODE`]).
#[must_use]
pub fn reserved(msg_id: &str) -> Option<NoticeKind> {
    NoticeKind::ALL
        .iter()
        .copied()
        .find(|kind| msg_id.starts_with(kind.prefix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_mints_an_id_in_its_own_reserved_namespace() {
        for &kind in NoticeKind::ALL {
            let id = kind.id("x");
            assert!(reserved(&id).is_some(), "{id}");
            let prefix = kind.prefix();
            assert!(
                prefix.ends_with([':', '-']) && prefix.len() > 2,
                "{prefix:?} must end at a separator"
            );
            let unique = NoticeKind::ALL
                .iter()
                .filter(|other| other.prefix() == prefix)
                .count();
            assert_eq!(unique, 1, "{prefix:?} is owned by one kind");
        }
    }

    #[test]
    fn caller_minted_ids_are_never_reserved() {
        // The CLI's UUIDv7, the shim's and native sends' `<hex>-<hex>`, and
        // the extensions' ids.
        for id in [
            "01a1bc63-7f29-7c60-824b-7f1278a8c7b3",
            "16cb6e3db15866e4-0000000000000005",
            "ba9aa7b34b6212ad-000000000000001b",
            "msg-1",
            "pij-swift-kestrel-1",
            "compaction",
            "outputs-1",
        ] {
            assert_eq!(reserved(id), None, "{id}");
        }
    }
}
