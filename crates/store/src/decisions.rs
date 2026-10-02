//! Durable decision lifecycle on the existing shared orchestration pool.
use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};
use crate::spine::{append_generated_in_transaction, append_in_transaction};
use pij_core::decisions::{Decision, DecisionState};
use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId, Seq};
use serde_json::json;
use sqlx::{Row, SqliteConnection};

/// A decision plus its persisted answer receipt, absent before completion.
#[derive(Clone, Debug)]
pub struct StoredDecision {
    /// Historical parent is retained here; live views project responsibility.
    pub decision: Decision,
    /// Stable receipt for idempotent answered readback.
    pub answer_seq: Option<Seq>,
}

impl SqliteOrchestration {
    /// Commit the question row and its opened event atomically under the bus.
    pub async fn open_decision_committed(
        &self,
        id: &str,
        asker: &SeatId,
        parent: Option<&SeatId>,
        question: &str,
        at: u64,
    ) -> Result<(Event, Decision)> {
        require_current_schema(&self.pool).await?;
        if id.trim().is_empty() || question.trim().is_empty() {
            return Err(refusal("E-RS-ARG", id));
        }
        let mut decision = Decision {
            id: id.to_string(),
            asked_by: asker.clone(),
            parent: parent.cloned(),
            question: question.to_string(),
            state: DecisionState::Open,
            asked_at: at,
            question_seq: Seq(0),
            answer: None,
            answered_by: None,
            answered_at: None,
            answer_msg_id: None,
        };
        let pool = self.pool.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let draft = decision_event(&decision, &decision.asked_by, "opened", at);
        let event = append_generated_in_transaction(&mut tx, draft, |seq| {
            decision.question_seq = seq;
            decision_event(&decision, &decision.asked_by, "opened", at).payload
        })
        .await?;
        sqlx::query("INSERT INTO decisions (id,asked_by,parent,question,state,asked_at,question_seq) VALUES (?1,?2,?3,?4,'open',?5,?6)")
            .bind(&decision.id).bind(decision.asked_by.as_str()).bind(decision.parent.as_ref().map(SeatId::as_str)).bind(&decision.question)
            .bind(sql_i64(at,"asked_at")?).bind(sql_i64(decision.question_seq.0,"question_seq")?)
            .execute(&mut *tx).await.map_err(adapter_error)?;
        tx.commit().await.map_err(adapter_error)?;
        Ok((event, decision))
        }).await
    }

    /// Read a durable question without fabricating a missing receipt.
    pub async fn get_decision(&self, id: &str) -> Result<Option<StoredDecision>> {
        require_current_schema(&self.pool).await?;
        let row = sqlx::query("SELECT * FROM decisions WHERE id=?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?;
        row.map(decode).transpose()
    }

    /// Bulk query for fleet projection; caller applies current-parent filters.
    pub async fn list_decisions(&self) -> Result<Vec<Decision>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM decisions ORDER BY asked_at,id")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(|row| decode(row).map(|stored| stored.decision))
            .collect()
    }

    /// Persist immutable outbound linkage BEFORE transport admission. A rejected
    /// delivery leaves this open prepared row visible and safely repeatable.
    pub async fn prepare_decision_answer(
        &self,
        id: &str,
        actor: &SeatId,
        answer: &str,
        msg_id: Option<&str>,
    ) -> Result<StoredDecision> {
        require_current_schema(&self.pool).await?;
        if answer.trim().is_empty() {
            return Err(refusal("E-RS-ARG", id));
        }
        let pool = self.pool.clone();
        let id = id.to_owned();
        let actor = actor.clone();
        let answer = answer.to_owned();
        let msg_id = msg_id.map(str::to_owned);
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let mut stored = read_in_transaction(&mut tx, &id).await?;
        if stored.decision.answer.is_some() {
            if stored.decision.answer.as_deref() != Some(answer.as_str())
                || stored.decision.answered_by.as_ref() != Some(&actor)
                || stored.decision.answer_msg_id.as_deref() != msg_id.as_deref()
            {
                return Err(refusal("E-RS-ANSWER-CONFLICT", &id));
            }
        } else {
            sqlx::query("UPDATE decisions SET answer=?2,answered_by=?3,answer_msg_id=?4 WHERE id=?1 AND state='open'")
                .bind(&id).bind(&answer).bind(actor.as_str()).bind(msg_id.as_deref()).execute(&mut *tx).await.map_err(adapter_error)?;
            stored.decision.answer = Some(answer);
            stored.decision.answered_by = Some(actor);
            stored.decision.answer_msg_id = msg_id;
        }
        tx.commit().await.map_err(adapter_error)?;
        Ok(stored)
        }).await
    }

    /// Replace only a terminal, non-delivered intent; queued/claimed work cannot
    /// be withdrawn. The audit, new message identity, and prepared row commit together.
    pub async fn supersede_decision_answer_committed(
        &self,
        id: &str,
        actor: &SeatId,
        answer: &str,
        at: u64,
    ) -> Result<(Event, StoredDecision)> {
        require_current_schema(&self.pool).await?;
        if answer.trim().is_empty() {
            return Err(refusal("E-RS-ARG", id));
        }
        let pool = self.pool.clone();
        let id = id.to_owned();
        let actor = actor.clone();
        let answer = answer.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let mut stored = read_in_transaction(&mut tx, &id).await?;
        let old_msg_id = stored
            .decision
            .answer_msg_id
            .as_deref()
            .ok_or_else(|| refusal("E-RS-ANSWER-CONFLICT", &id))?;
        ensure_answer_terminal(&mut tx, &stored.decision, old_msg_id).await?;
        let superseded = json!({"answer_msg_id":old_msg_id,"answered_by":stored.decision.answered_by,"answer":stored.decision.answer});
        stored.decision.state = DecisionState::Open;
        stored.decision.answer = Some(answer);
        stored.decision.answered_by = Some(actor.clone());
        stored.decision.answered_at = None;
        stored.answer_seq = None;
        let draft = decision_event(&stored.decision, &actor, "answer-superseded", at);
        let event = append_generated_in_transaction(&mut tx,draft,|seq| {
            stored.decision.answer_msg_id = Some(format!("{id}-answer-{}",seq.0));
            json!({"actor":actor,"action":"answer-superseded","record":stored.decision,"superseded":superseded}).to_string()
        }).await?;
        sqlx::query("UPDATE decisions SET state='open',answer=?2,answered_by=?3,answer_msg_id=?4,answered_at=NULL,answer_seq=NULL WHERE id=?1")
            .bind(&id).bind(stored.decision.answer.as_deref()).bind(actor.as_str()).bind(&stored.decision.answer_msg_id)
            .execute(&mut *tx).await.map_err(adapter_error)?;
        tx.commit().await.map_err(adapter_error)?;
        Ok((event, stored))
        }).await
    }

    /// Close only after durable transport acceptance (or an authorized self
    /// ruling). State and decision.answered are all-or-nothing under the bus.
    pub async fn answer_decision_committed(
        &self,
        id: &str,
        actor: &SeatId,
        expected_msg_id: Option<&str>,
        at: u64,
    ) -> Result<(Event, Decision)> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        let actor = actor.clone();
        let expected_msg_id = expected_msg_id.map(str::to_owned);
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let stored = read_in_transaction(&mut tx, &id).await?;
            let mut decision = stored.decision;
            if decision.state != DecisionState::Open
                || decision.answer.is_none()
                || decision.answered_by.as_ref() != Some(&actor)
                || decision.answer_msg_id.as_deref() != expected_msg_id.as_deref()
            {
                return Err(refusal("E-RS-ANSWER-CONFLICT", &id));
            }
            decision.state = DecisionState::Answered;
            decision.answered_at = Some(at);
            let mut event = decision_event(&decision, &actor, "answered", at);
            let seq = append_in_transaction(&mut tx, &event).await?;
            sqlx::query(
                "UPDATE decisions SET state='answered',answered_at=?2,answer_seq=?3 WHERE id=?1",
            )
            .bind(&id)
            .bind(sql_i64(at, "answered_at")?)
            .bind(sql_i64(seq.0, "answer_seq")?)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            event.seq = Some(seq);
            Ok((event, decision))
        })
        .await
    }

    /// Append a receipt for the latest done naming the requested assignment, or
    /// latest done overall when unscoped. Caller resolves current-parent authority
    /// under the bus lock.
    pub async fn verify_done_committed(
        &self,
        actor: &SeatId,
        target: &SeatId,
        assignment: Option<&str>,
        at: u64,
    ) -> Result<(Event, serde_json::Value)> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let actor = actor.clone();
        let target = target.clone();
        let assignment = assignment.map(str::to_owned);
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let history = sqlx::query("SELECT seq,at,payload FROM spine_events WHERE seat=?1 AND kind='report.state' ORDER BY seq DESC")
            .bind(target.as_str()).fetch_all(&mut *tx).await.map_err(adapter_error)?;
        let mut latest = None;
        let mut saw_done = false;
        for row in history {
            let payload: String = row.try_get("payload").map_err(adapter_error)?;
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload) else {
                continue;
            };
            if payload["state"] == "done" {
                saw_done = true;
                if assignment.as_deref().is_some_and(|id| payload["assignment_id"].as_str() != Some(id)) {
                    continue;
                }
                latest = Some((
                    sql_u64(row.try_get("seq").map_err(adapter_error)?, "done_seq")?,
                    sql_u64(row.try_get("at").map_err(adapter_error)?, "done_at")?,
                ));
                break;
            }
        }
        let (done_seq, done_at) = latest.ok_or_else(|| match assignment.as_deref() {
            Some(id) if saw_done => refusal("E-RS-DONE-ASSIGNMENT", id),
            _ => refusal("E-RS-NO-DONE", target.as_str()),
        })?;
        if let Some(assignment) = assignment.as_deref() {
            let task =
                sqlx::query("SELECT node_id,opened_at,closed_at FROM task_assignments WHERE id=?1")
                    .bind(assignment)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(adapter_error)?
                    .ok_or_else(|| refusal("E-RS-DONE-ASSIGNMENT", assignment))?;
            let node: String = task.try_get("node_id").map_err(adapter_error)?;
            let opened = sql_u64(
                task.try_get("opened_at").map_err(adapter_error)?,
                "opened_at",
            )?;
            let closed: Option<i64> = task.try_get("closed_at").map_err(adapter_error)?;
            if node != target.as_str()
                || opened > done_at
                || closed
                    .map(|value| sql_u64(value, "closed_at"))
                    .transpose()?
                    .is_some_and(|closed| closed < done_at)
            {
                return Err(refusal("E-RS-DONE-ASSIGNMENT", assignment));
            }
        }
        let mut receipt = json!({"seat":target,"verified_by":actor,"done_seq":done_seq});
        if let Some(assignment) = assignment.as_deref() {
            receipt["assignment_id"] = json!(assignment);
        }
        let mut event = Event {
            seq: None,
            v: 1,
            at,
            kind: "state-verified".into(),
            seat: Some(target.clone()),
            payload: receipt.to_string(),
        };
        let seq = append_in_transaction(&mut tx, &event).await?;
        tx.commit().await.map_err(adapter_error)?;
        event.seq = Some(seq);
        Ok((event, receipt))
        }).await
    }
}

async fn read_in_transaction(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<StoredDecision> {
    let row = sqlx::query("SELECT * FROM decisions WHERE id=?1")
        .bind(id)
        .fetch_optional(connection)
        .await
        .map_err(adapter_error)?
        .ok_or_else(|| refusal("E-RS-NO-DECISION", id))?;
    decode(row)
}

/// Read all delivery vetoes inside the same write transaction as replacement.
/// Historical delivery survives the bounded delivered-id cache's eviction.
async fn ensure_answer_terminal(
    connection: &mut SqliteConnection,
    decision: &Decision,
    msg_id: &str,
) -> Result<()> {
    let delivered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE dedupe_key=?1 AND state='done') OR EXISTS(SELECT 1 FROM spine_events WHERE kind='delivery.outcome' AND seat=?2 AND CASE WHEN json_valid(payload) THEN json_extract(payload,'$.msg_id')=?1 AND json_extract(payload,'$.outcome.outcome')='delivered' ELSE 0 END)"
    ).bind(msg_id).bind(decision.asked_by.as_str()).fetch_one(&mut *connection).await.map_err(adapter_error)?;
    if delivered {
        return Err(answer_delivery_refusal(
            "E-RS-ANSWER-DELIVERED",
            &decision.id,
            msg_id,
            "delivered",
        ));
    }
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM jobs WHERE dedupe_key=?1 AND state!='failed' ORDER BY id DESC LIMIT 1",
    )
    .bind(msg_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(adapter_error)?;
    if let Some(state) = state {
        return Err(answer_delivery_refusal(
            "E-RS-ANSWER-IN-TRANSIT",
            &decision.id,
            msg_id,
            &state,
        ));
    }
    let outcome: Option<String> = sqlx::query_scalar("SELECT COALESCE(json_extract(payload,'$.outcome.outcome'),'unknown') FROM spine_events WHERE kind='delivery.outcome' AND seat=?1 AND CASE WHEN json_valid(payload) THEN json_extract(payload,'$.msg_id')=?2 ELSE 0 END ORDER BY seq DESC LIMIT 1")
        .bind(decision.asked_by.as_str()).bind(msg_id).fetch_optional(&mut *connection).await.map_err(adapter_error)?;
    match outcome.as_deref() {
        Some("held") => {
            return Err(answer_delivery_refusal(
                "E-RS-ANSWER-IN-TRANSIT",
                &decision.id,
                msg_id,
                "held",
            ));
        }
        // A completed refusal is terminal even if its pre-injection marker remains.
        Some("refused") => return Ok(()),
        _ => {}
    }
    let reserved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM delivered_messages WHERE recipient=?1 AND msg_id=?2)",
    )
    .bind(decision.asked_by.as_str())
    .bind(msg_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(adapter_error)?;
    if reserved {
        return Err(answer_delivery_refusal(
            "E-RS-ANSWER-IN-TRANSIT",
            &decision.id,
            msg_id,
            "delivery-reserved",
        ));
    }
    Ok(())
}

fn answer_delivery_refusal(code: &str, decision: &str, msg_id: &str, state: &str) -> PijError {
    PijError::GovernanceRefused {
        code: code.to_string(),
        record: json!({"decision":decision,"answer_msg_id":msg_id,"job_state":state}).to_string(),
    }
}

fn decision_event(row: &Decision, actor: &SeatId, action: &str, at: u64) -> Event {
    Event {
        seq: None,
        v: 1,
        at,
        kind: format!("decision.{action}"),
        seat: Some(row.asked_by.clone()),
        payload: json!({"actor":actor,"action":action,"record":row}).to_string(),
    }
}

fn refusal(code: &str, id: &str) -> PijError {
    PijError::GovernanceRefused {
        code: code.to_string(),
        record: id.to_string(),
    }
}

fn decode(row: sqlx::sqlite::SqliteRow) -> Result<StoredDecision> {
    let state: String = row.try_get("state").map_err(adapter_error)?;
    let optional_at: Option<i64> = row.try_get("answered_at").map_err(adapter_error)?;
    let answer_seq: Option<i64> = row.try_get("answer_seq").map_err(adapter_error)?;
    Ok(StoredDecision {
        decision: Decision {
            id: row.try_get("id").map_err(adapter_error)?,
            asked_by: SeatId::from(
                row.try_get::<String, _>("asked_by")
                    .map_err(adapter_error)?,
            ),
            parent: row
                .try_get::<Option<String>, _>("parent")
                .map_err(adapter_error)?
                .map(SeatId::from),
            question: row.try_get("question").map_err(adapter_error)?,
            state: match state.as_str() {
                "open" => DecisionState::Open,
                "answered" => DecisionState::Answered,
                _ => return Err(refusal("E-RS-DECISION-STATE", &state)),
            },
            asked_at: sql_u64(row.try_get("asked_at").map_err(adapter_error)?, "asked_at")?,
            question_seq: Seq(sql_u64(
                row.try_get("question_seq").map_err(adapter_error)?,
                "question_seq",
            )?),
            answer: row.try_get("answer").map_err(adapter_error)?,
            answered_by: row
                .try_get::<Option<String>, _>("answered_by")
                .map_err(adapter_error)?
                .map(SeatId::from),
            answered_at: optional_at
                .map(|at| sql_u64(at, "answered_at"))
                .transpose()?,
            answer_msg_id: row.try_get("answer_msg_id").map_err(adapter_error)?,
        },
        answer_seq: answer_seq
            .map(|seq| sql_u64(seq, "answer_seq").map(Seq))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteSpine;
    use pij_core::ports::Spine;
    use pij_testkit::FreshStore;

    #[tokio::test]
    async fn decision_survives_reopen_and_prepared_answer_remains_open() {
        let fresh = FreshStore::new();
        let pool = crate::open(&fresh.path()).await.expect("open");
        let store = SqliteOrchestration::new(pool.clone());
        let asker = SeatId::from("worker");
        let parent = SeatId::from("parent");
        let (event, decision) = store
            .open_decision_committed("d1", &asker, Some(&parent), "which branch?", 100)
            .await
            .expect("open question");
        assert_eq!(event.seq, Some(decision.question_seq));
        let payload: serde_json::Value = serde_json::from_str(&event.payload).expect("event");
        assert_eq!(payload["record"]["question_seq"], decision.question_seq.0);
        store
            .prepare_decision_answer("d1", &parent, "main", Some("d1-answer"))
            .await
            .expect("prepare");
        pool.close().await;
        let pool = crate::open(&fresh.path()).await.expect("reopen");
        let store = SqliteOrchestration::new(pool.clone());
        let pending = store.get_decision("d1").await.expect("read").expect("row");
        assert_eq!(pending.decision.state, DecisionState::Open);
        assert_eq!(pending.decision.answer_msg_id.as_deref(), Some("d1-answer"));
        assert!(
            store
                .prepare_decision_answer("d1", &parent, "other", Some("d1-answer"))
                .await
                .is_err()
        );
        let (answered, row) = store
            .answer_decision_committed("d1", &parent, Some("d1-answer"), 101)
            .await
            .expect("accepted");
        assert_eq!(row.state, DecisionState::Answered);
        assert_eq!(
            store
                .get_decision("d1")
                .await
                .expect("read")
                .expect("row")
                .answer_seq,
            answered.seq
        );
        let events = SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .expect("events");
        assert_eq!(events, vec![event, answered]);
        pool.close().await;
    }

    #[tokio::test]
    async fn failed_answer_event_rolls_back_decision_closure() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool.clone());
        let actor = SeatId::from("worker");
        store
            .open_decision_committed("d1", &actor, None, "which branch?", 100)
            .await
            .expect("question");
        store
            .prepare_decision_answer("d1", &actor, "main", None)
            .await
            .expect("prepare");
        sqlx::query("CREATE TRIGGER reject_answer BEFORE INSERT ON spine_events WHEN NEW.kind='decision.answered' BEGIN SELECT RAISE(ABORT,'injected event failure'); END").execute(&pool).await.expect("inject");
        assert!(
            store
                .answer_decision_committed("d1", &actor, None, 101)
                .await
                .is_err()
        );
        let row = store.get_decision("d1").await.expect("read").expect("row");
        assert_eq!(row.decision.state, DecisionState::Open);
        assert!(row.answer_seq.is_none());
    }

    #[tokio::test]
    async fn verification_checks_explicit_assignment_node_and_done_association() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool.clone());
        let spine = SqliteSpine::new(pool.clone());
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contract");
        let mut task: pij_core::orchestration::TaskAssignment =
            serde_json::from_value(fixtures["fixture_context"]["records"]["task"].clone())
                .expect("task");
        task.opened_at = 1;
        task.project = None;
        store.open_task(&task).await.expect("task");
        let actor = SeatId::from("parent");
        let done = spine
            .append(Event {
                seq: None,
                v: 1,
                at: 2,
                kind: "report.state".into(),
                seat: Some(task.node_id.clone()),
                payload: json!({"state":"done"}).to_string(),
            })
            .await
            .expect("unscoped done");
        assert!(
            store
                .verify_done_committed(&actor, &task.node_id, Some(&task.id), 3)
                .await
                .is_err()
        );
        let (_, verified) = store
            .verify_done_committed(&actor, &task.node_id, None, 3)
            .await
            .expect("seat-only");
        assert_eq!(verified["done_seq"], done.0);
        let scoped = spine
            .append(Event {
                seq: None,
                v: 1,
                at: 4,
                kind: "report.state".into(),
                seat: Some(task.node_id.clone()),
                payload: json!({"state":"done","assignment_id":task.id}).to_string(),
            })
            .await
            .expect("associated done");
        let (_, verified) = store
            .verify_done_committed(&actor, &task.node_id, Some(&task.id), 5)
            .await
            .expect("scoped verify");
        assert_eq!(verified["done_seq"], scoped.0);
        assert!(
            store
                .verify_done_committed(&actor, &task.node_id, Some("different-task"), 5)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn generated_payload_failure_rolls_back_draft_and_decision() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool.clone());
        sqlx::query("CREATE TRIGGER reject_payload BEFORE UPDATE OF payload ON spine_events BEGIN SELECT RAISE(ABORT,'injected finalization failure'); END").execute(&pool).await.expect("inject");
        assert!(
            store
                .open_decision_committed("d1", &SeatId::from("worker"), None, "which branch?", 100)
                .await
                .is_err()
        );
        assert!(store.get_decision("d1").await.expect("read").is_none());
        assert!(
            SqliteSpine::new(pool)
                .tail(None, Seq(0))
                .await
                .expect("events")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn superseded_intent_and_audit_survive_reopen() {
        let fresh = FreshStore::new();
        let pool = crate::open(&fresh.path()).await.expect("open");
        let store = SqliteOrchestration::new(pool.clone());
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-events.json"
        ))
        .expect("events");
        let fixture = fixtures["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|row| row["id"] == "decision-answer-superseded")
            .expect("supersede fixture");
        let mut expected = fixture["decoded_payload"].clone();
        let id = expected["record"]["id"].as_str().expect("id").to_string();
        let asker = SeatId::from(expected["record"]["asked_by"].as_str().expect("asker"));
        let previous = SeatId::from(
            expected["superseded"]["answered_by"]
                .as_str()
                .expect("old parent"),
        );
        let actor = SeatId::from(expected["actor"].as_str().expect("current parent"));
        let (_, question) = store
            .open_decision_committed(
                &id,
                &asker,
                Some(&previous),
                expected["record"]["question"].as_str().expect("question"),
                expected["record"]["asked_at"].as_u64().expect("time"),
            )
            .await
            .expect("question");
        store
            .prepare_decision_answer(
                &id,
                &previous,
                expected["superseded"]["answer"]
                    .as_str()
                    .expect("old answer"),
                expected["superseded"]["answer_msg_id"].as_str(),
            )
            .await
            .expect("old intent");
        let (event, prepared) = store
            .supersede_decision_answer_committed(
                &id,
                &actor,
                expected["record"]["answer"].as_str().expect("new answer"),
                1788739200005,
            )
            .await
            .expect("supersede");
        let new_id = format!("{id}-answer-{}", event.seq.expect("real seq").0);
        expected["record"]["question_seq"] = json!(question.question_seq);
        expected["record"]["answer_msg_id"] = json!(new_id);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&event.payload).expect("payload"),
            expected
        );
        assert_eq!(prepared.decision.state, DecisionState::Open);
        assert!(prepared.answer_seq.is_none());
        pool.close().await;
        let pool = crate::open(&fresh.path()).await.expect("reopen");
        let store = SqliteOrchestration::new(pool.clone());
        let reopened = store.get_decision(&id).await.expect("read").expect("row");
        assert_eq!(
            reopened.decision.answer_msg_id.as_deref(),
            Some(new_id.as_str())
        );
        assert_eq!(reopened.decision.answered_by, Some(actor));
        assert_eq!(
            SqliteSpine::new(pool.clone())
                .tail(None, question.question_seq)
                .await
                .expect("history"),
            vec![event]
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn supersede_row_failure_rolls_back_new_event_and_linkage() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool.clone());
        let actor = SeatId::from("parent");
        let (_, question) = store
            .open_decision_committed(
                "d1",
                &SeatId::from("worker"),
                Some(&actor),
                "which branch?",
                100,
            )
            .await
            .expect("question");
        store
            .prepare_decision_answer("d1", &actor, "old", Some("d1-answer"))
            .await
            .expect("intent");
        sqlx::query("CREATE TRIGGER reject_supersede BEFORE UPDATE OF answer ON decisions WHEN NEW.answer != OLD.answer BEGIN SELECT RAISE(ABORT,'injected replacement failure'); END").execute(&pool).await.expect("inject");
        assert!(
            store
                .supersede_decision_answer_committed("d1", &actor, "new", 101)
                .await
                .is_err()
        );
        let row = store.get_decision("d1").await.expect("read").expect("row");
        assert_eq!(row.decision.answer.as_deref(), Some("old"));
        assert_eq!(row.decision.answer_msg_id.as_deref(), Some("d1-answer"));
        assert_eq!(
            store.spine_head().await.expect("head"),
            question.question_seq
        );
    }

    #[tokio::test]
    async fn obsolete_answer_completion_cannot_close_replacement_intent() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool);
        let actor = SeatId::from("parent");
        store
            .open_decision_committed(
                "d1",
                &SeatId::from("worker"),
                Some(&actor),
                "which branch?",
                100,
            )
            .await
            .expect("question");
        store
            .prepare_decision_answer("d1", &actor, "old", Some("d1-answer"))
            .await
            .expect("intent");
        let (_, prepared) = store
            .supersede_decision_answer_committed("d1", &actor, "new", 101)
            .await
            .expect("supersede");
        assert!(
            store
                .answer_decision_committed("d1", &actor, Some("d1-answer"), 102)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .get_decision("d1")
                .await
                .expect("read")
                .expect("row")
                .decision
                .state,
            DecisionState::Open
        );
        store
            .answer_decision_committed(
                "d1",
                &actor,
                prepared.decision.answer_msg_id.as_deref(),
                103,
            )
            .await
            .expect("current completion");
    }

    #[tokio::test]
    async fn spine_head_observes_empty_and_latest_committed_sequence() {
        let pool = crate::open("").await.expect("memory");
        let store = SqliteOrchestration::new(pool.clone());
        assert_eq!(store.spine_head().await.expect("empty head"), Seq(0));
        let spine = SqliteSpine::new(pool);
        for at in [1, 2] {
            let seq = spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at,
                    kind: "head-proof".into(),
                    seat: None,
                    payload: "{}".into(),
                })
                .await
                .expect("append");
            assert_eq!(store.spine_head().await.expect("head"), seq);
        }
    }
}
