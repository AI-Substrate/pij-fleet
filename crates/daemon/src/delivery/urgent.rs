//! `pij send --urgent` (plan 214, AC3 and AC4).
//!
//! A normal message to a seat in a long turn waits for that turn to end (F25).
//! An urgent one asks for the recipient's next tool boundary instead. How
//! depends on the harness; where there is no tool-boundary path the send goes
//! through as a normal one, and the receipt says why.

use pij_core::model::{DeliveryOutcome, Harness, Receipt, SeatDescriptor};

/// The refusal for `--urgent` with `--fyi`: one asks for mid-turn delivery,
/// the other holds the message for the next turn.
pub const E_URGENT_FYI: &str = "E-RS-URGENT-FYI";

/// The spine event written once per urgent send: sender, recipient, msg_id,
/// harness and outcome, so the PA can count urgent sends per sender.
pub const URGENT_EVENT_KIND: &str = "send.urgent";

/// How an urgent send reaches its recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrgentRoute {
    /// Native Copilot: the pij extension hands it over at the next tool call
    /// (`onPostToolUse` / `onPostToolUseFailure` additional context).
    ToolBoundary,
    /// OMP and Pi already steer every message into a running turn.
    Steered,
    /// Claude's socket transport already delivers between tool calls.
    Socket,
    /// No tool-boundary path: delivered as a normal send, at turn end.
    Downgraded(String),
}

impl UrgentRoute {
    /// The route for `seat`, from its registration alone (no IO).
    pub fn for_seat(seat: &SeatDescriptor) -> Self {
        match seat.harness {
            Harness::Copilot if seat.native_extension_delivery => Self::ToolBoundary,
            Harness::Copilot => Self::Downgraded(
                "this Copilot seat has no native pij extension, so nothing reads it at a tool call"
                    .into(),
            ),
            Harness::Omp | Harness::Pi => Self::Steered,
            Harness::Claude if seat.cross_session_inbound_accept == Some(true) => Self::Socket,
            Harness::Claude => Self::Downgraded(
                "this Claude seat has no inbound socket, so pij can only type at its turn end"
                    .into(),
            ),
            Harness::Codex => Self::Downgraded(
                "Codex has no tool-boundary hook pij can use; delivered at its turn end".into(),
            ),
        }
    }

    /// The spine event's outcome word.
    pub fn outcome(&self) -> &'static str {
        match self {
            Self::ToolBoundary | Self::Socket => "injected",
            Self::Steered => "steered",
            Self::Downgraded(_) => "downgraded",
        }
    }

    /// Whether the delivery must be marked for the tool-boundary take: only a
    /// native Copilot recipient whose message was queued.
    pub fn marks(&self, receipt: &Receipt) -> bool {
        *self == Self::ToolBoundary && matches!(receipt.outcome, DeliveryOutcome::Queued { .. })
    }

    /// The receipt warning for a downgrade, naming the harness and why.
    pub fn warning(&self, seat: &SeatDescriptor) -> Option<String> {
        match self {
            Self::Downgraded(why) => Some(format!(
                "--urgent downgraded for {} ({}): {why}. Sent as a normal message.",
                seat.id, seat.harness
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use pij_core::model::{Harness, SeatDescriptor};

    use super::UrgentRoute;

    fn seat(harness: Harness) -> SeatDescriptor {
        SeatDescriptor::new("pij-to", harness, "/tmp")
    }

    #[test]
    fn each_harness_gets_its_route_and_only_a_downgrade_warns() {
        let mut copilot = seat(Harness::Copilot);
        assert!(matches!(
            UrgentRoute::for_seat(&copilot),
            UrgentRoute::Downgraded(_)
        ));
        copilot.native_extension_delivery = true;
        assert_eq!(UrgentRoute::for_seat(&copilot), UrgentRoute::ToolBoundary);
        assert_eq!(
            UrgentRoute::for_seat(&seat(Harness::Omp)),
            UrgentRoute::Steered
        );
        assert_eq!(
            UrgentRoute::for_seat(&seat(Harness::Pi)),
            UrgentRoute::Steered
        );
        let mut claude = seat(Harness::Claude);
        assert!(matches!(
            UrgentRoute::for_seat(&claude),
            UrgentRoute::Downgraded(_)
        ));
        claude.cross_session_inbound_accept = Some(true);
        assert_eq!(UrgentRoute::for_seat(&claude), UrgentRoute::Socket);
        let codex = seat(Harness::Codex);
        let route = UrgentRoute::for_seat(&codex);
        assert_eq!(route.outcome(), "downgraded");
        let warning = route.warning(&codex).expect("a downgrade warns");
        assert!(
            warning.contains("pij-to") && warning.contains("codex"),
            "{warning}"
        );
        assert_eq!(UrgentRoute::Steered.warning(&codex), None);
    }
}
