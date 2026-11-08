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

/// One kind of daemon-authored message, by the id prefix it owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// A background job's batches, final turn and cold notices.
    PijBg,
    /// A message parked after its delivery attempts ran out.
    ParkNotice,
    /// A message held at the hold limit.
    HoldNotice,
    /// The prime has no live PA.
    PaMissing,
    /// The PA is dormant.
    PaDormant,
    /// The PA does not answer.
    PaUnresponsive,
    /// Seats over the size limit, told to the prime.
    LargeSeats,
    /// A PA watchdog round.
    PaWatchdog,
    /// The prime is active, told to the PA.
    PrimeActive,
    /// A usage limit has reset.
    LimitReset,
    /// A steer control was held.
    SteerControl,
    /// A watchdog control was held.
    WatchdogControl,
    /// The routing table, after a compact.
    RoutesAfterCompact,
    /// A role request was closed.
    RequestClosed,
    /// A role request was withdrawn (its role retired or merged).
    RequestWithdrawn,
    /// The memory turn before a compact or close.
    MemoryTurn,
    /// A seat a compaction left idle is woken.
    CompactionWake,
    /// Its parent is told about that wake.
    CompactionWakeFyi,
    /// A role request queued for a busy role.
    RoleQueue,
    /// A role request's time-limit check-in.
    RoleLimitCheckIn,
    /// A role request's time-limit notice.
    RoleLimitNotice,
    /// Held FYIs flushed into one turn.
    FyiFlush,
    /// A seat idle while holding work.
    IdleNudge,
    /// Its prime is told about that nudge.
    IdleNudgeFyi,
    /// A seat stalled on a modal.
    ModalStall,
    /// An output was written.
    Output,
    /// A seat running twice.
    DoubleSeat,
    /// A chore was defined or ran.
    Chore,
    /// An inbound Telegram message.
    Telegram,
}

impl NoticeKind {
    /// Every kind, so the reserved table cannot miss one.
    pub const ALL: [Self; 29] = [
        Self::PijBg,
        Self::ParkNotice,
        Self::HoldNotice,
        Self::PaMissing,
        Self::PaDormant,
        Self::PaUnresponsive,
        Self::LargeSeats,
        Self::PaWatchdog,
        Self::PrimeActive,
        Self::LimitReset,
        Self::SteerControl,
        Self::WatchdogControl,
        Self::RoutesAfterCompact,
        Self::RequestClosed,
        Self::RequestWithdrawn,
        Self::MemoryTurn,
        Self::CompactionWake,
        Self::CompactionWakeFyi,
        Self::RoleQueue,
        Self::RoleLimitCheckIn,
        Self::RoleLimitNotice,
        Self::FyiFlush,
        Self::IdleNudge,
        Self::IdleNudgeFyi,
        Self::ModalStall,
        Self::Output,
        Self::DoubleSeat,
        Self::Chore,
        Self::Telegram,
    ];

    /// The id prefix this kind owns. These bytes are the dedupe keys of rows
    /// already in live stores: change one and its notices dedupe afresh.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::PijBg => "pij-bg:",
            Self::ParkNotice => "park-notice-",
            Self::HoldNotice => "hold-notice-",
            Self::PaMissing => "pa-missing:",
            Self::PaDormant => "pa-dormant:",
            Self::PaUnresponsive => "pa-unresponsive:",
            Self::LargeSeats => "large-seats:",
            Self::PaWatchdog => "pa-watchdog:",
            Self::PrimeActive => "prime-active:",
            Self::LimitReset => "limit-reset:",
            Self::SteerControl => "steer-control:",
            Self::WatchdogControl => "watchdog-control:",
            Self::RoutesAfterCompact => "routes-after-compact:",
            Self::RequestClosed => "request-closed-",
            Self::RequestWithdrawn => "request-withdrawn-",
            Self::MemoryTurn => "memory-turn-",
            Self::CompactionWake => "compaction-wake:",
            Self::CompactionWakeFyi => "compaction-wake-fyi:",
            Self::RoleQueue => "role-queue-",
            Self::RoleLimitCheckIn => "role-limit-checkin-",
            Self::RoleLimitNotice => "role-limit-notice-",
            Self::FyiFlush => "fyi-flush-",
            Self::IdleNudge => "idle-nudge:",
            Self::IdleNudgeFyi => "idle-nudge-fyi:",
            Self::ModalStall => "modal-stall:",
            Self::Output => "output-",
            Self::DoubleSeat => "double-seat-",
            Self::Chore => "chore-",
            Self::Telegram => "telegram-",
        }
    }

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
        .into_iter()
        .find(|kind| msg_id.starts_with(kind.prefix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_mints_an_id_in_its_own_reserved_namespace() {
        for kind in NoticeKind::ALL {
            let id = kind.id("x");
            assert!(reserved(&id).is_some(), "{id}");
            let prefix = kind.prefix();
            assert!(
                prefix.ends_with([':', '-']) && prefix.len() > 2,
                "{prefix:?} must end at a separator"
            );
            let unique = NoticeKind::ALL
                .into_iter()
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
