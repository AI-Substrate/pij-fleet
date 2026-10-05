//! Pure message-routing policy.
//!
//! Runtime delivery belongs in `pij-daemon`; this module decides whether the
//! facts already known about a seat permit a transport attempt, require durable
//! queueing, or make future delivery impossible.

use crate::error::{PijError, Result};
use crate::model::{Msg, SeatDescriptor, SeatId};
use serde::{Deserialize, Serialize};

/// Quiet interval after the last human keystroke before send-keys delivery resumes.
/// The daemon alone applies the environment override and publishes the value.
pub const DEFAULT_TYPING_GRACE_MS: u64 = 60_000;

/// Durable evidence that a pending delivery is waiting on human typing, not read.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HeldEvent {
    /// The sender's stable message id.
    pub msg_id: String,
    /// Stable wire reason, currently `human-typing`.
    pub reason: String,
    /// Recipient whose extension owns the claim.
    pub seat: SeatId,
    /// Unix milliseconds of the last observed human keystroke.
    pub since_ms: u64,
}

/// Evidence that the extension is releasing a hold; this is not a delivery ack.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleasedEvent {
    /// Unix milliseconds when the extension decided to release.
    pub at_ms: u64,
    /// The sender's stable message id.
    pub msg_id: String,
    /// Recipient whose extension owns the claim.
    pub seat: SeatId,
}

/// Refuse terminal recovery when queue events would cross persistence authorities.
///
/// # Errors
/// `E-RS-INBOX-AUTHORITY-SPLIT` for a real queue paired with a fake spine.
pub fn require_recovery_authority(shared: bool) -> Result<()> {
    if !shared {
        return Err(PijError::GovernanceRefused {
            code: "E-RS-INBOX-AUTHORITY-SPLIT".into(),
            record: r#"{"queue_backend":"real","spine_backend":"fake"}"#.into(),
        });
    }
    Ok(())
}

/// Build the canonical sender receipt and recipient-observable parking fact.
/// The queue commits these events in the same transaction as terminal state.
///
/// # Errors
/// A stored delivery payload cannot be decoded as a message.
pub fn parked_events(
    job_id: crate::model::JobId,
    job: &crate::model::Job,
    evidence: &crate::ports::ParkingEvidence<'_>,
) -> Result<Vec<crate::model::Event>> {
    let message: Msg = serde_json::from_str(&job.payload).map_err(|error| PijError::Adapter {
        adapter: "delivery/parking".into(),
        message: format!("invalid parked delivery payload: {error}"),
    })?;
    let event = |kind: &str, payload: serde_json::Value| crate::model::Event {
        seq: None,
        v: 1,
        at: evidence.at,
        kind: kind.into(),
        seat: Some(message.from.clone()),
        payload: payload.to_string(),
    };
    Ok(vec![
        event(
            "delivery.outcome",
            serde_json::json!({
                "msg_id":message.msg_id,
                "outcome":{"outcome":"refused","reason":evidence.outcome.as_str()},
                "transport":"extension-stream",
            }),
        ),
        event(
            "delivery.parked",
            serde_json::json!({
                "messageId":message.msg_id, "jobId":job_id, "recipient":job.serial_key,
                "outcome":evidence.outcome, "reason":evidence.reason,
            }),
        ),
    ])
}

/// Maximum complete framed message accepted by pane typing.
///
/// Socket transports remain unrestricted; an oversized socketless frame stays
/// queued for a named inbox pull instead of racing a machine-speed timeout.
pub const MAX_TYPED_FRAME_BYTES: usize = 8 * 1024;

/// Delivery deferrals publish at most once per minute per job, regardless of reason.
/// The first attempt publishes immediately; later samples summarize reason changes.
pub const DEFERRAL_EVENT_INTERVAL_MS: u64 = 60_000;

/// Build the sampled held fact from the queue's durable diagnostic authority.
#[must_use]
pub fn delivery_deferral_event(
    deferral: &crate::model::DeliveryDeferral,
    seat: &SeatId,
    draft_sha: Option<&str>,
    at: u64,
    reason_changes: u64,
) -> crate::model::Event {
    crate::model::Event {
        seq: None,
        v: 1,
        at,
        kind: "delivery.held".into(),
        seat: Some(seat.clone()),
        payload: serde_json::json!({
            "job_id": deferral.job_id,
            "msg_id": deferral.msg_id,
            "seat": seat,
            "reason": deferral.reason,
            "since_ms": deferral.since_ms,
            "deferral_count": deferral.count,
            "reason_changes": reason_changes,
            "last_edit_at": null,
            "remaining_ms": null,
            "draft_sha": draft_sha,
        })
        .to_string(),
    }
}

/// Why the interaction gate deferred a pane-bound delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryDeferralReason {
    /// No tap owned by this daemon exists yet.
    TapUnowned,
    /// A recognized composer contains non-whitespace text.
    ComposerBusy,
    /// Tmux mode or recent interaction evidence says a person is typing.
    HumanTyping,
    /// The pane or composer could not be recognized safely.
    Unrecognized,
}

impl DeliveryDeferralReason {
    /// Stable wire spelling for queued receipts and state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TapUnowned => "tap-unowned",
            Self::ComposerBusy => "composer-busy",
            Self::HumanTyping => "human-typing",
            Self::Unrecognized => "unrecognized",
        }
    }
}

/// Why a message must be durably queued instead of attempted now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueReason {
    /// The recipient is currently processing another turn.
    Busy,
    /// The recipient exists but has not bound to a process yet.
    PreBind,
    /// The selected transport cannot currently reach the recipient.
    Unreachable,
}

impl QueueReason {
    /// Stable wire spelling for queued receipts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::PreBind => "pre-bind",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The route selected from registry and transport facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryRoute {
    /// Try the transport now.
    Transport,
    /// Persist the message for later delivery.
    Queue(QueueReason),
    /// The registry row is a post-mortem; this seat will never receive again.
    Tombstoned {
        /// The recorded reason, when one was retained.
        reason: Option<String>,
    },
}

/// The queue kind owned by one recipient's inbox.
///
/// Recipient-specific kinds let `Queue::claim` select one inbox without taking
/// another seat's work. Producers and consumers must call this function rather
/// than formatting the open string independently.
pub fn delivery_kind(seat: &SeatId) -> String {
    format!("delivery:{seat}")
}

/// Whether a body contains a terminal control character that a stock composer
/// cannot represent losslessly. Linefeed remains ordinary body text.
#[must_use]
pub fn body_requires_pull(body: &str) -> bool {
    body.chars().any(|ch| ch.is_control() && ch != '\n')
}

/// The mechanism selected for one durably accepted message.
///
/// This is deliberately above [`crate::ports::Transport`]: a transport can
/// answer whether it can carry a message, but it cannot select the PTY or
/// pane-typing fallback when it answers `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryRung {
    /// A paneless seat owns its queue through an in-process pull consumer.
    Observe,
    /// Submit the message's explicitly tagged command through the PTY.
    Pty,
    /// Carry a body through the selected transport.
    Socket,
    /// Keep an unrepresentable control-bearing body queued and tell the pane to pull it.
    ControlBodyPull,
    /// Submit the full framed body to an idle pane.
    TypedBody,
    /// No safe delivery mechanism is available; leave the row queued.
    Queue,
}

/// Select the delivery ladder rung from already-observed facts.
///
/// `human_typing` vetoes send-keys mechanisms only. Socket bodies bypass composer
/// permission because their transport never modifies or submits the human draft.
/// A command is identified only by [`Msg::command`]; a body beginning with `/`
/// remains a body. Typed-body delivery and the named control-body pull exception
/// are available only when the composer is idle.
#[must_use]
pub fn select_rung(
    recipient: &SeatDescriptor,
    msg: &Msg,
    socket_available: bool,
    human_typing: bool,
    composer_idle: bool,
) -> DeliveryRung {
    if recipient.pane.is_none() {
        return DeliveryRung::Observe;
    }
    if msg.command.is_none() && socket_available {
        return DeliveryRung::Socket;
    }
    if human_typing {
        return DeliveryRung::Queue;
    }
    if msg.command.is_some() {
        return DeliveryRung::Pty;
    }
    if !composer_idle {
        return DeliveryRung::Queue;
    }
    if body_requires_pull(&msg.body) {
        return DeliveryRung::ControlBodyPull;
    }
    DeliveryRung::TypedBody
}

/// Select a route without performing IO.
///
/// Tombstones dominate every transient fact: a dead seat is never given an
/// immortal queue row. Pre-bind and transport unreachability are temporary and
/// therefore queue honestly. Self-reported status never gates the inbox.
///
/// **`Working` is deliberately NOT decided here.** It used to be — a busy
/// recipient queued before the transport was ever consulted — and u-uds found at
/// its ack gate that this forecloses the entire reason the socket transport
/// exists: Claude accepts a socket frame BETWEEN TOOL CALLS MID-TURN, which is to
/// say precisely when the seat is `Working`. A correct socket transport could not
/// have overcome a prefilter that ran before it; the unit would have passed every
/// one of its own tests and delivered nothing.
///
/// "Busy" was a pty-era fact: typing races a live composer, so a pty transport
/// declines a working seat — and that is the PTY's judgement to make, in
/// `can_deliver`, where the transport that knows its own medium answers for it.
/// The queue reason is still [`QueueReason::Busy`] when a working seat is
/// declined; what moved is WHO decides, not what the operator is told.
///
/// # Errors
/// [`PijError::SelfAddressed`] when a seat addresses itself.
pub fn route(
    from: &SeatId,
    from_machine: Option<&str>,
    recipient: &SeatDescriptor,
    transport_reachable: bool,
) -> Result<DeliveryRoute> {
    // Self-addressing is a question about ADDRESSES, not names. `alice@laptop`
    // sending to the local `alice` is a stranger with a common name, and the bare
    // comparison refused it — which is the exact collision R7-AMEND-1 exists to
    // solve, still live inside the function that had to answer it (u-federation).
    //
    // A recipient reached through this function is always LOCAL, so a sender with
    // any machine is by construction not it.
    if from == &recipient.id && from_machine.is_none() {
        return Err(PijError::SelfAddressed { seat: from.clone() });
    }
    Ok(route_control(recipient, transport_reachable))
}

/// Route an already-authorized control, including controls addressed to self.
/// Tombstones, pre-bind and transport reachability remain authoritative.
#[must_use]
pub fn route_control(recipient: &SeatDescriptor, transport_reachable: bool) -> DeliveryRoute {
    if recipient.tombstoned_at.is_some() {
        return DeliveryRoute::Tombstoned {
            reason: recipient.tombstone_reason.clone(),
        };
    }
    if recipient.proc.is_none() {
        return DeliveryRoute::Queue(QueueReason::PreBind);
    }
    if !transport_reachable {
        return DeliveryRoute::Queue(QueueReason::Unreachable);
    }
    DeliveryRoute::Transport
}

#[cfg(test)]
mod tests {
    /// Review F02/F01 (plan 164): a parked FORWARDED message was sent by no
    /// seat here, so its parking facts must not be attributed to a local seat
    /// that merely shares the remote sender's bare name; the origin travels in
    /// the payload instead.
    #[test]
    fn a_parked_forwarded_message_is_attributed_to_no_local_seat() {
        let msg = crate::model::Msg {
            from: crate::model::SeatId::from("pij-sender"),
            from_machine: Some("laptop".to_string()),
            to: crate::model::SeatId::from("pij-reader"),
            body: "hi".to_string(),
            msg_id: "m-1".to_string(),
            in_reply_to: None,
            command: None,
        };
        let job = crate::model::Job {
            kind: "delivery:pij-reader".to_string(),
            serial_key: "pij-reader".to_string(),
            payload: serde_json::to_string(&msg).expect("payload"),
            dedupe_key: "m-1".to_string(),
            dedupe_origin: Some("laptop".to_string()),
            attempt: 0,
        };
        let events = super::parked_events(
            crate::model::JobId(1),
            &job,
            &crate::ports::ParkingEvidence {
                outcome: crate::model::DeliveryFailure::OperatorReleased,
                reason: "test",
                at: 1,
            },
        )
        .expect("events");
        for event in events {
            assert_eq!(event.seat, None, "{}: {}", event.kind, event.payload);
            let payload: serde_json::Value = serde_json::from_str(&event.payload).expect("json");
            assert_eq!(payload["from_machine"], "laptop", "{}", event.kind);
        }
    }

    use super::{DeliveryRoute, DeliveryRung, QueueReason, route, select_rung};
    use crate::error::PijError;
    use crate::model::{
        Harness, Msg, ProcIdentity, SeatDescriptor, SeatId, SemanticState, SystemState,
    };

    fn bound_recipient() -> SeatDescriptor {
        let mut seat = SeatDescriptor::new("pij-recipient", Harness::Omp, "/abs/tree");
        seat.proc = Some(ProcIdentity {
            pid: 7,
            proc_start: 11,
        });
        seat
    }

    #[test]
    fn self_reported_hold_still_routes_incoming_messages() {
        let mut seat = bound_recipient();
        seat.semantic_state = Some(SemanticState::Hold);
        assert_eq!(
            route(&SeatId::from("pij-sender"), None, &seat, true),
            Ok(DeliveryRoute::Transport),
            "a status report cannot revoke the inbox needed for the awaited ruling"
        );
    }

    #[test]
    fn controls_route_to_self_without_weakening_body_or_tombstone_policy() {
        let mut seat = bound_recipient();
        assert!(matches!(
            route(&seat.id, None, &seat, true),
            Err(PijError::SelfAddressed { .. })
        ));
        assert_eq!(super::route_control(&seat, true), DeliveryRoute::Transport);
        seat.semantic_state = Some(SemanticState::Hold);
        assert_eq!(super::route_control(&seat, true), DeliveryRoute::Transport);
        seat.tombstoned_at = Some(1);
        assert!(matches!(
            super::route_control(&seat, true),
            DeliveryRoute::Tombstoned { .. }
        ));
    }

    #[test]
    fn receipt_honesty_table_selects_only_recoverable_queue_routes() {
        let from = SeatId::from("pij-sender");
        let mut seat = bound_recipient();
        assert_eq!(
            route(&from, None, &seat, true),
            Ok(DeliveryRoute::Transport)
        );

        // A WORKING seat still routes to the transport: whether a busy recipient
        // can be reached is the transport's judgement, because only it knows
        // whether its medium interrupts (a socket does, a pty does not). The
        // operator still hears `Busy` — the caller supplies that reason when the
        // transport declines a working seat.
        seat.state = SystemState::Working;
        assert_eq!(
            route(&from, None, &seat, true),
            Ok(DeliveryRoute::Transport)
        );
        assert_eq!(
            route(&from, None, &seat, false),
            Ok(DeliveryRoute::Queue(QueueReason::Unreachable))
        );

        seat.state = SystemState::Idle;
        seat.proc = None;
        assert_eq!(
            route(&from, None, &seat, true),
            Ok(DeliveryRoute::Queue(QueueReason::PreBind))
        );

        seat.proc = Some(ProcIdentity {
            pid: 7,
            proc_start: 11,
        });
        seat.semantic_state = Some(SemanticState::Hold);
        assert_eq!(
            route(&from, None, &seat, true),
            Ok(DeliveryRoute::Transport)
        );

        seat.semantic_state = None;
        assert_eq!(
            route(&from, None, &seat, false),
            Ok(DeliveryRoute::Queue(QueueReason::Unreachable))
        );

        seat.tombstoned_at = Some(42);
        seat.tombstone_reason = Some("process exited".to_string());
        assert_eq!(
            route(&from, None, &seat, false),
            Ok(DeliveryRoute::Tombstoned {
                reason: Some("process exited".to_string())
            })
        );
    }

    fn message(body: &str, command: Option<&str>) -> Msg {
        Msg {
            from: SeatId::from("pij-sender"),
            to: SeatId::from("pij-recipient"),
            body: body.to_string(),
            msg_id: "m-1".to_string(),
            from_machine: None,
            in_reply_to: None,
            command: command.map(str::to_string),
        }
    }

    #[test]
    fn delivery_ladder_enumerates_every_rung_in_order() {
        let body = message("hello", None);
        let command = message("display text is not the command tag", Some("compact"));
        let mut paneless = bound_recipient();
        paneless.pane = None;
        assert_eq!(
            select_rung(&paneless, &command, true, false, false),
            DeliveryRung::Observe,
            "a paneless seat pulls; it is never pushed to"
        );

        let mut paned = bound_recipient();
        paned.pane = Some("%7".to_string());
        assert_eq!(
            select_rung(&paned, &command, true, false, true),
            DeliveryRung::Pty,
            "an explicitly tagged command takes the PTY even when a socket exists"
        );
        assert_eq!(
            select_rung(&paned, &body, true, false, true),
            DeliveryRung::Socket,
            "a body uses an available socket"
        );
        assert_eq!(
            select_rung(&paned, &body, false, false, true),
            DeliveryRung::TypedBody,
            "a socketless body is typed only when the composer is idle"
        );
        assert_eq!(
            select_rung(&paned, &body, false, false, false),
            DeliveryRung::Queue,
            "without a socket or an idle composer, the body stays queued"
        );
    }

    #[test]
    fn terminal_control_body_uses_socket_or_named_pull_never_typing() {
        let mut recipient = bound_recipient();
        recipient.pane = Some("%control".to_string());
        let body = message("before\u{1b}[201~after", None);

        assert_eq!(
            select_rung(&recipient, &body, true, false, true),
            DeliveryRung::Socket,
            "a socket can carry the original control bytes"
        );
        assert_eq!(
            select_rung(&recipient, &body, false, false, true),
            DeliveryRung::ControlBodyPull,
            "a stock composer cannot represent the bracketed-paste terminator"
        );
        assert_eq!(
            select_rung(&recipient, &body, false, false, false),
            DeliveryRung::Queue,
            "even the safe pointer waits for an idle composer"
        );
        assert_eq!(
            select_rung(
                &recipient,
                &message("line one\nline two", None),
                false,
                false,
                true,
            ),
            DeliveryRung::TypedBody,
            "linefeed remains representable body text"
        );
        assert_eq!(
            select_rung(&recipient, &message("tab\tvalue", None), false, false, true,),
            DeliveryRung::ControlBodyPull,
            "OMP expands tab bytes, so tab takes the lossless pull exception"
        );
    }

    #[test]
    fn socketless_idle_pane_selects_typed_body() {
        let mut recipient = bound_recipient();
        recipient.pane = Some("%typed".to_string());

        assert_eq!(
            select_rung(&recipient, &message("full body", None), false, false, true),
            DeliveryRung::TypedBody,
            "an idle pane without a socket receives the body through tmux"
        );
    }

    #[test]
    fn human_typing_only_vetoes_send_keys_delivery() {
        let mut recipient = bound_recipient();
        recipient.pane = Some("%7".to_string());
        let body = message("hello", None);
        for composer_idle in [false, true] {
            assert_eq!(
                select_rung(&recipient, &body, true, true, composer_idle),
                DeliveryRung::Socket,
                "socket delivery cannot overwrite the human's composer"
            );
            for msg in [
                &body,
                &message("control\u{1b}", None),
                &message("", Some("compact")),
            ] {
                assert_eq!(
                    select_rung(&recipient, msg, false, true, composer_idle),
                    DeliveryRung::Queue,
                    "every send-keys rung retains the typing veto"
                );
            }
            assert_eq!(
                select_rung(
                    &recipient,
                    &message("", Some("compact")),
                    true,
                    true,
                    composer_idle
                ),
                DeliveryRung::Queue,
                "an available socket does not bypass command typing safety"
            );
        }
    }

    #[test]
    fn only_the_command_field_selects_the_pty() {
        let mut recipient = bound_recipient();
        recipient.pane = Some("%7".to_string());
        assert_eq!(
            select_rung(&recipient, &message("/compact", None), true, false, true),
            DeliveryRung::Socket,
            "a leading slash in a body is still body text"
        );
        assert_eq!(
            select_rung(
                &recipient,
                &message("not /compact", Some("compact")),
                true,
                false,
                true,
            ),
            DeliveryRung::Pty,
            "the bare command tag, not display text, selects the PTY"
        );
    }

    #[test]
    fn self_delivery_is_refused_by_address_not_by_name() {
        let seat = bound_recipient();

        // Local, same name: still refused. This is the case the rule is for.
        assert_eq!(
            route(&seat.id, None, &seat, true),
            Err(PijError::SelfAddressed {
                seat: seat.id.clone()
            })
        );

        // SAME NAME, DIFFERENT MACHINE: a stranger with a common name, and it must
        // route. Two machines each holding a seat called `alice` is ordinary, and
        // the bare-name comparison refused `alice@laptop -> alice` as if it were
        // someone talking to themselves — inside the very function R7-AMEND-1 was
        // ratified to unblock (u-federation).
        assert_eq!(
            route(&seat.id, Some("laptop"), &seat, true),
            Ok(DeliveryRoute::Transport)
        );
    }
}
