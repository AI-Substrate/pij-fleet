//! Durable questions and answer authority; no transport or persistence.
use crate::model::{SeatDescriptor, SeatId, Seq};
use serde::{Deserialize, Serialize};

/// Whether a question has an accepted answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionState {
    /// Awaiting an authorized answer.
    Open,
    /// Answer committed after acceptance, or self-ruling without delivery.
    Answered,
}

/// Persisted question. At rest `parent` is the parent at creation; live views
/// replace it with current responsibility without rewriting history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Stable question identity.
    pub id: String,
    /// Seat that owns the question.
    pub asked_by: SeatId,
    /// Historical parent at rest, current responsible parent in live views.
    pub parent: Option<SeatId>,
    /// Original question text.
    pub question: String,
    /// Durable answer lifecycle.
    pub state: DecisionState,
    /// Creation epoch milliseconds.
    pub asked_at: u64,
    /// Real decision.opened spine sequence.
    pub question_seq: Seq,
    /// Prepared answer may exist while delivery refusal keeps state open.
    pub answer: Option<String>,
    /// Resolved actor of the prepared/accepted answer.
    pub answered_by: Option<SeatId>,
    /// Accepted answer epoch milliseconds, absent while open.
    pub answered_at: Option<u64>,
    /// Stable delivery id; self-answers deliberately have no message.
    pub answer_msg_id: Option<String>,
}

/// Current parent alone grants parent authority; role/prime visibility does not.
pub fn may_answer(actor: &SeatId, asker: &SeatDescriptor) -> bool {
    actor == &asker.id || asker.parent.as_ref() == Some(actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    #[test]
    fn answer_authority_follows_reparenting_not_historical_parent_or_role() {
        let mut asker = SeatDescriptor::new("worker", Harness::Omp, "/work");
        asker.parent = Some(SeatId::from("old"));
        assert!(may_answer(&SeatId::from("old"), &asker));
        asker.parent = Some(SeatId::from("new"));
        assert!(!may_answer(&SeatId::from("old"), &asker));
        assert!(may_answer(&SeatId::from("new"), &asker));
        assert!(may_answer(&asker.id, &asker));
        assert!(!may_answer(&SeatId::from("prime"), &asker));
    }
}
