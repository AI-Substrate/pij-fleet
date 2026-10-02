//! Pure anomaly detection over an immutable store view.
//!
//! The composition edge reads the Registry and Spine once, decodes their rows
//! into [`AnomalyView`], then hands that snapshot to these synchronous
//! detectors. Core never performs IO and never infers a fact the snapshot did
//! not carry.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::decisions::{Decision, DecisionState};
use crate::model::{Card, Event, SeatDescriptor, SeatId, SemanticState, Seq};

/// Durable Spine kind written when an anomaly occurrence is acknowledged.
pub const ANOMALY_ACK_KIND: &str = "anomaly.ack";
/// Durable Spine kind written when an anomaly occurrence is cleared.
pub const ANOMALY_CLEAR_KIND: &str = "anomaly.clear";

/// Status anomaly policy; the separate command stderr nudge stays ten minutes.
pub const STATUS_STALE_MS: u64 = 30 * 60 * 1_000;

/// The three anomaly kinds with existing or explicitly named consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnomalyKind {
    /// A status card is older than the configured threshold.
    StatusStale,
    /// A delivered dispatch has not received its durable packet acknowledgement.
    DeliveredUnackedStale,
    /// A done declaration has no later verification.
    UnverifiedDone,
    /// An unanswered durable question belongs to its current responsible seat.
    OpenQuestion,
    /// The recorded process incarnation is no longer alive.
    DeadSeat,
}

impl AnomalyKind {
    /// Stable spelling consumed by the CLI and existing TypeScript readers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StatusStale => "status-stale",
            Self::DeliveredUnackedStale => "delivered-unacked-stale",
            Self::UnverifiedDone => "unverified-done",
            Self::OpenQuestion => "open-question",
            Self::DeadSeat => "dead-seat",
        }
    }
}

impl fmt::Display for AnomalyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a visible anomaly occurrence has been acknowledged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnomalyStatus {
    /// Nobody has acknowledged this occurrence.
    Open,
    /// An `anomaly.ack` Spine event names this exact occurrence.
    Acknowledged,
}

/// One actionable anomaly row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anomaly {
    /// Stable kind consumed by the anomaly command and alerting surface.
    pub kind: AnomalyKind,
    /// Seat the row concerns.
    pub seat: SeatId,
    /// What was measured, including the source evidence that made it true.
    pub observable: String,
    /// Exact command an operator or agent can run next.
    pub remediation_line: String,
    /// Stable identity of this condition occurrence.
    ///
    /// Clear and acknowledgement events key on this value. It changes only
    /// when the source evidence changes, never merely because another scan ran.
    pub occurrence: String,
    /// Current acknowledgement state. Cleared rows are omitted entirely.
    pub status: AnomalyStatus,
    /// Human detail, preserving the established relay vocabulary.
    pub detail: String,
    /// Real committed sequences that prove the condition.
    pub evidence: Vec<Seq>,
    /// Present only when the done event actually names an assignment.
    pub assignment_id: Option<String>,
    /// Stable referenced governance record.
    pub record_ref: Option<String>,
    /// Measured elapsed milliseconds, when the condition is age-based.
    pub age_ms: Option<u64>,
}

/// Frozen governance wire shape, borrowing detector evidence rather than copying it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnomalyWire<'a> {
    /// Existing kind literal.
    pub kind: AnomalyKind,
    /// Seat responsible for this row.
    pub node_id: &'a SeatId,
    /// Optional actual assignment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignment_id: Option<&'a str>,
    /// Human detail.
    pub detail: &'a str,
    /// Source evidence.
    pub evidence: &'a [Seq],
    /// Referenced governance record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_ref: Option<&'a str>,
    /// Measured age.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    /// Exact established relay wrapper.
    pub relay: String,
}

impl Anomaly {
    /// Render only the public governance fields from this detector occurrence.
    pub fn wire(&self) -> AnomalyWire<'_> {
        let assignment_suffix = self
            .assignment_id
            .as_ref()
            .map_or_else(String::new, |id| format!(" (assignment {id})"));
        let evidence = if self.evidence.is_empty() {
            "none".to_string()
        } else {
            self.evidence
                .iter()
                .map(|seq| seq.0.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        AnomalyWire {
            kind: self.kind,
            node_id: &self.seat,
            assignment_id: self.assignment_id.as_deref(),
            detail: &self.detail,
            evidence: &self.evidence,
            record_ref: self.record_ref.as_deref(),
            age_ms: self.age_ms,
            relay: format!(
                "\u{26a0}\u{fe0f} anomaly {} on {}{}: {} — evidence: spine {}",
                self.kind, self.seat, assignment_suffix, self.detail, evidence
            ),
        }
    }
}

/// A confirmed dead/recycled process together with its registry source event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadSeatFact {
    /// The affected seat.
    pub seat: SeatId,
    /// Real seat.put sequence for this incarnation.
    pub seq: Seq,
}

/// A Spine-derived activity observation for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityFact {
    /// Seat that emitted the event.
    pub seat: SeatId,
    /// Event timestamp in milliseconds since the Unix epoch.
    pub at: u64,
    /// Durable source sequence.
    pub seq: Seq,
}

/// A typed delivered-dispatch view assembled by the composition edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchFact {
    /// Recipient expected to acknowledge the packet.
    pub seat: SeatId,
    /// Durable dispatch id consumed by `pij ack`.
    pub dispatch_id: String,
    /// Lowercase SHA-256 required by `pij ack`.
    pub packet_sha256: String,
    /// When delivery landed, in milliseconds since the Unix epoch.
    pub delivered_at: u64,
    /// Spine sequence that proves this delivery occurrence.
    pub delivered_seq: Seq,
    /// Whether a durable brief acknowledgement followed this delivery.
    pub acknowledged: bool,
}

/// A typed done/verification projection assembled from report Spine events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoneFact {
    /// Seat that declared the assignment done.
    pub seat: SeatId,
    /// Assignment whose done claim needs verification.
    pub assignment_id: Option<String>,
    /// Spine sequence of the done declaration.
    pub done_seq: Seq,
    /// Latest verification sequence, when one exists.
    pub verified_seq: Option<Seq>,
    /// Exact done event referenced by the verification, never inferred by time.
    pub verified_done_seq: Option<Seq>,
}

/// Configured detector thresholds.
///
/// Defaults preserve the existing anomaly surface, but every scan receives a
/// value so callers can configure policy rather than inheriting hidden timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnomalyThresholds {
    /// Maximum card age before `status-stale` fires.
    pub status_stale_ms: u64,
    /// Maximum delivered-but-unacknowledged age before the dispatch row fires.
    pub delivered_unacked_stale_ms: u64,
}

impl Default for AnomalyThresholds {
    fn default() -> Self {
        Self {
            status_stale_ms: STATUS_STALE_MS,
            delivered_unacked_stale_ms: 15 * 60 * 1_000,
        }
    }
}

/// Immutable facts a detector may consult.
///
/// The view deliberately carries typed projections for facts the Registry does
/// not own. In particular, system state is never treated as proof of activity,
/// and a dispatch row cannot exist without the id and packet hash its runnable
/// remediation requires.
pub struct AnomalyView<'a> {
    /// Scan time in milliseconds since the Unix epoch.
    pub now_ms: u64,
    /// Caller-selected timing policy.
    pub thresholds: AnomalyThresholds,
    /// Current Registry rows.
    pub seats: &'a [SeatDescriptor],
    /// Latest and historical status cards read from the Spine.
    pub cards: &'a [Card],
    /// Activity observations decoded from the Spine.
    pub activity: &'a [ActivityFact],
    /// Delivered dispatch projections.
    pub dispatches: &'a [DispatchFact],
    /// Done/verification projections.
    pub done: &'a [DoneFact],
    /// Durable acknowledgement and clear events.
    pub dispositions: &'a [Event],
    /// Open decisions with current responsibility projected by the composition edge.
    pub decisions: &'a [Decision],
    /// Positive liveness evidence; probe errors are never dead facts.
    pub dead: &'a [DeadSeatFact],
}

/// A pure synchronous detector over an immutable store snapshot.
pub trait Detector {
    /// Return every currently visible row owned by this detector.
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly>;
}

/// Detects cards older than the configured threshold.
pub struct StatusStaleDetector;

impl Detector for StatusStaleDetector {
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly> {
        let mut rows = Vec::new();
        for seat in view
            .seats
            .iter()
            .filter(|seat| seat.tombstoned_at.is_none())
            .filter(|seat| {
                !matches!(
                    seat.semantic_state,
                    Some(
                        SemanticState::Waiting
                            | SemanticState::Hold
                            | SemanticState::Blocked
                            | SemanticState::Question
                    )
                )
            })
        {
            let Some(card) = view
                .cards
                .iter()
                .filter(|card| card.seat == seat.id)
                .max_by_key(|card| (card.at, card.seq))
            else {
                continue;
            };
            let Some(source_seq) = card.seq else {
                // A row that cannot name stable source evidence cannot stay
                // cleared. The composition edge must supply persisted cards.
                continue;
            };
            let age_ms = view.now_ms.saturating_sub(card.at);
            if age_ms <= view.thresholds.status_stale_ms {
                continue;
            }

            // Activity remains explicit spine evidence. Parked declarations
            // suppress card drift by ruling, independently of the stderr nudge.
            let latest_activity = view
                .activity
                .iter()
                .filter(|fact| fact.seat == seat.id)
                .max_by_key(|fact| (fact.at, fact.seq));
            let activity_observable = latest_activity.map_or_else(
                || "latest_activity=absent".to_string(),
                |fact| {
                    format!(
                        "latest_activity_at={} latest_activity_seq={}",
                        fact.at, fact.seq.0
                    )
                },
            );

            rows.push(Anomaly {
                kind: AnomalyKind::StatusStale,
                seat: seat.id.clone(),
                observable: format!(
                    "card_at={} card_seq={} age_ms={} threshold_ms={} {}",
                    card.at,
                    source_seq.0,
                    age_ms,
                    view.thresholds.status_stale_ms,
                    activity_observable
                ),
                remediation_line: format!(
                    "pij send {} \"Refresh your stale card with: pij report now '<what I just did>' '<what is next>'\"",
                    seat.id
                ),
                occurrence: occurrence(AnomalyKind::StatusStale, &seat.id, source_seq),
                status: AnomalyStatus::Open,
                detail: format!("'{}' has been working for {}min since its card was last updated (threshold {}min) — consumers render now/next as CURRENT, so a stale card actively misinforms. If '{}' is waiting on something with no known end, it should declare a parked state: pij report state waiting|hold|blocked|question (parked seats never flag). Otherwise it should update its card: pij report now \"<what I just did>\" \"<what's next>\" — note that refreshing a card resets this timer WITHOUT changing the wait, so a parked seat that reports instead of declaring will be asked again every threshold.", seat.id, age_ms / 60_000, view.thresholds.status_stale_ms / 60_000, seat.id),
                evidence: vec![source_seq], assignment_id: None, record_ref: None, age_ms: Some(age_ms),
            });
        }
        apply_dispositions(rows, view.dispositions)
    }
}

/// Detects delivered dispatches whose packet acknowledgement never arrived.
pub struct DeliveredUnackedDetector;

impl Detector for DeliveredUnackedDetector {
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly> {
        let rows = view
            .dispatches
            .iter()
            .filter(|fact| !fact.acknowledged)
            .filter(|fact| is_lower_hex_sha256(&fact.packet_sha256))
            .filter_map(|fact| {
                let age_ms = view.now_ms.saturating_sub(fact.delivered_at);
                (age_ms > view.thresholds.delivered_unacked_stale_ms).then(|| Anomaly {
                    kind: AnomalyKind::DeliveredUnackedStale,
                    seat: fact.seat.clone(),
                    observable: format!(
                        "dispatch={} delivered_at={} delivered_seq={} age_ms={} threshold_ms={} acknowledged=false",
                        fact.dispatch_id,
                        fact.delivered_at,
                        fact.delivered_seq.0,
                        age_ms,
                        view.thresholds.delivered_unacked_stale_ms
                    ),
                    remediation_line: format!(
                        "pij ack {} --packet-sha {}",
                        fact.dispatch_id, fact.packet_sha256
                    ),
                    occurrence: occurrence(
                        AnomalyKind::DeliveredUnackedStale,
                        &fact.seat,
                        fact.delivered_seq,
                    ),
                    status: AnomalyStatus::Open,
                    detail: format!("dispatch:{} remains delivered-unacked — delivery landed but no durable brief ack followed", fact.dispatch_id),
                    evidence: vec![fact.delivered_seq], assignment_id: None,
                    record_ref: Some(format!("dispatch:{}", fact.dispatch_id)), age_ms: Some(age_ms),
                })
            })
            .collect();
        apply_dispositions(rows, view.dispositions)
    }
}

/// Detects done declarations with no later verification.
pub struct UnverifiedDoneDetector;

impl Detector for UnverifiedDoneDetector {
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly> {
        let rows = view
            .done
            .iter()
            .filter(|fact| fact.verified_done_seq != Some(fact.done_seq) || fact.verified_seq.is_none_or(|seq| seq <= fact.done_seq))
            .map(|fact| Anomaly {
                kind: AnomalyKind::UnverifiedDone,
                seat: fact.seat.clone(),
                observable: format!(
                    "assignment={} done_seq={} verified_seq={}",
                    fact.assignment_id.as_deref().unwrap_or("absent"),
                    fact.done_seq.0,
                    fact.verified_seq
                        .map_or_else(|| "absent".to_string(), |seq| seq.0.to_string())
                ),
                remediation_line: fact.assignment_id.as_ref().map_or_else(
                    || format!("pij report verify {}", fact.seat),
                    |id| format!("pij report verify {} --assignment {}", fact.seat, id),
                ),
                occurrence: occurrence(AnomalyKind::UnverifiedDone, &fact.seat, fact.done_seq),
                status: AnomalyStatus::Open,
                detail: fact.assignment_id.as_ref().map_or_else(
                    || format!("{} declared done with no verify — done is a claim until verified", fact.seat),
                    |id| format!("assignment '{id}' declared done by {} with no verify — done is a claim until verified", fact.seat),
                ),
                evidence: vec![fact.done_seq], assignment_id: fact.assignment_id.clone(), record_ref: None, age_ms: None,
            })
            .collect();
        apply_dispositions(rows, view.dispositions)
    }
}

/// Run the existing detectors and the two governance detectors on one snapshot.
pub fn scan_all(view: &AnomalyView<'_>) -> Vec<Anomaly> {
    let mut rows = StatusStaleDetector.scan(view);
    rows.extend(DeliveredUnackedDetector.scan(view));
    rows.extend(UnverifiedDoneDetector.scan(view));
    rows.extend(OpenQuestionDetector.scan(view));
    rows.extend(DeadSeatDetector.scan(view));
    rows
}

/// Questions stay visible independently of card or semantic state changes.
pub struct OpenQuestionDetector;

impl Detector for OpenQuestionDetector {
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly> {
        let mut rows = Vec::new();
        for decision in view
            .decisions
            .iter()
            .filter(|row| row.state == DecisionState::Open)
        {
            let seat = decision.parent.as_ref().unwrap_or(&decision.asked_by);
            let age_ms = view.now_ms.saturating_sub(decision.asked_at);
            for kind in [AnomalyKind::OpenQuestion, AnomalyKind::StatusStale] {
                if kind == AnomalyKind::StatusStale && age_ms <= view.thresholds.status_stale_ms {
                    continue;
                }
                let detail = if kind == AnomalyKind::OpenQuestion {
                    format!(
                        "decision '{}' asked by {} is unanswered",
                        decision.id, decision.asked_by
                    )
                } else {
                    format!(
                        "decision '{}' asked by {} has awaited {} for {}min (threshold {}min) — answer with pij answer {} \"<ruling>\"",
                        decision.id,
                        decision.asked_by,
                        seat,
                        age_ms / 60_000,
                        view.thresholds.status_stale_ms / 60_000,
                        decision.id
                    )
                };
                rows.push(Anomaly {
                    kind,
                    seat: seat.clone(),
                    observable: format!(
                        "decision={} question_seq={} age_ms={age_ms}",
                        decision.id, decision.question_seq.0
                    ),
                    remediation_line: format!("pij answer {} \"<ruling>\"", decision.id),
                    occurrence: occurrence(kind, seat, decision.question_seq),
                    status: AnomalyStatus::Open,
                    detail,
                    evidence: vec![decision.question_seq],
                    assignment_id: None,
                    record_ref: Some(format!("decision:{}", decision.id)),
                    age_ms: Some(age_ms),
                });
            }
        }
        apply_dispositions(rows, view.dispositions)
    }
}

/// Only positively observed dead/recycled incarnations become anomaly rows.
pub struct DeadSeatDetector;

impl Detector for DeadSeatDetector {
    fn scan(&self, view: &AnomalyView<'_>) -> Vec<Anomaly> {
        apply_dispositions(
            view.dead
                .iter()
                .map(|fact| Anomaly {
                    kind: AnomalyKind::DeadSeat,
                    seat: fact.seat.clone(),
                    observable: format!(
                        "seat={} registry_seq={} process_incarnation=dead",
                        fact.seat, fact.seq.0
                    ),
                    remediation_line: "pij-rs reap --dry-run".to_string(),
                    occurrence: occurrence(AnomalyKind::DeadSeat, &fact.seat, fact.seq),
                    status: AnomalyStatus::Open,
                    detail:
                        "recorded process incarnation is dead; inspect with pij-rs reap --dry-run"
                            .to_string(),
                    evidence: vec![fact.seq],
                    assignment_id: None,
                    record_ref: None,
                    age_ms: None,
                })
                .collect(),
            view.dispositions,
        )
    }
}

/// Build the durable acknowledgement event for one exact occurrence.
pub fn acknowledge_event(anomaly: &Anomaly, actor: &SeatId, at: u64) -> Event {
    disposition_event(ANOMALY_ACK_KIND, anomaly, actor, at)
}

/// Build the durable clear event for one exact occurrence.
pub fn clear_event(anomaly: &Anomaly, actor: &SeatId, at: u64) -> Event {
    disposition_event(ANOMALY_CLEAR_KIND, anomaly, actor, at)
}

#[derive(Serialize, Deserialize)]
struct DispositionPayload {
    occurrence: String,
    actor: SeatId,
}

fn disposition_event(kind: &str, anomaly: &Anomaly, actor: &SeatId, at: u64) -> Event {
    let payload = serde_json::to_string(&DispositionPayload {
        occurrence: anomaly.occurrence.clone(),
        actor: actor.clone(),
    })
    .expect("serializing strings and a SeatId cannot fail");
    Event {
        seq: None,
        v: 1,
        at,
        kind: kind.to_string(),
        seat: Some(anomaly.seat.clone()),
        payload,
    }
}

fn apply_dispositions(mut rows: Vec<Anomaly>, events: &[Event]) -> Vec<Anomaly> {
    let mut acknowledged = BTreeSet::new();
    let mut cleared = BTreeSet::new();
    for event in events {
        let target = match event.kind.as_str() {
            ANOMALY_ACK_KIND => &mut acknowledged,
            ANOMALY_CLEAR_KIND => &mut cleared,
            _ => continue,
        };
        if let Ok(payload) = serde_json::from_str::<DispositionPayload>(&event.payload) {
            target.insert(payload.occurrence);
        }
    }

    rows.retain(|row| !cleared.contains(&row.occurrence));
    for row in &mut rows {
        if acknowledged.contains(&row.occurrence) {
            row.status = AnomalyStatus::Acknowledged;
        }
    }
    rows
}

fn occurrence(kind: AnomalyKind, seat: &SeatId, source_seq: Seq) -> String {
    format!("{}:{}:{}", kind.as_str(), seat, source_seq.0)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Harness, SemanticState};

    const NOW: u64 = 2_000_000;

    fn seat(id: &str, state: Option<SemanticState>) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/abs/worktree");
        seat.semantic_state = state;
        seat
    }

    fn card(id: &str, at: u64, seq: u64) -> Card {
        Card {
            seat: SeatId::from(id),
            did: "implemented detector".to_string(),
            next: "run proof".to_string(),
            at,
            seq: Some(Seq(seq)),
        }
    }

    fn empty_view<'a>(
        seats: &'a [SeatDescriptor],
        cards: &'a [Card],
        activity: &'a [ActivityFact],
        dispatches: &'a [DispatchFact],
        done: &'a [DoneFact],
        dispositions: &'a [Event],
    ) -> AnomalyView<'a> {
        AnomalyView {
            now_ms: NOW,
            thresholds: AnomalyThresholds {
                status_stale_ms: 10 * 60 * 1_000,
                delivered_unacked_stale_ms: 15 * 60 * 1_000,
            },
            seats,
            cards,
            activity,
            dispatches,
            done,
            dispositions,
            decisions: &[],
            dead: &[],
        }
    }

    #[test]
    fn status_staleness_uses_thirty_minutes_and_suppresses_parked_states() {
        let seats = [seat("pij-worker", None)];
        let cards = [card("pij-worker", NOW - 1_800_001, 100)];
        let mut view = empty_view(&seats, &cards, &[], &[], &[], &[]);
        view.thresholds = AnomalyThresholds::default();
        let rows = StatusStaleDetector.scan(&view);
        assert_eq!(rows.len(), 1);
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("canonical contract");
        assert_eq!(
            serde_json::to_value(rows[0].wire()).expect("wire row"),
            fixtures["rows"]["anomalies"][0]
        );
        let boundary_cards = [card("pij-worker", NOW - 1_800_000, 101)];
        view.cards = &boundary_cards;
        assert!(StatusStaleDetector.scan(&view).is_empty());
        for state in [
            SemanticState::Waiting,
            SemanticState::Hold,
            SemanticState::Blocked,
            SemanticState::Question,
        ] {
            let parked = [seat("pij-worker", Some(state))];
            let mut view = empty_view(&parked, &cards, &[], &[], &[], &[]);
            view.thresholds = AnomalyThresholds::default();
            assert!(StatusStaleDetector.scan(&view).is_empty(), "{state:?}");
        }
    }

    #[test]
    fn delivered_unacked_uses_the_ts_kind_and_emits_the_exact_ack_command() {
        let dispatches = [DispatchFact {
            seat: SeatId::from("pij-coder"),
            dispatch_id: "dispatch-17".to_string(),
            packet_sha256: "a".repeat(64),
            delivered_at: NOW - 15 * 60 * 1_000 - 1,
            delivered_seq: Seq(51),
            acknowledged: false,
        }];
        let view = empty_view(&[], &[], &[], &dispatches, &[], &[]);

        let rows = DeliveredUnackedDetector.scan(&view);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind.as_str(), "delivered-unacked-stale");
        assert_eq!(
            rows[0].remediation_line,
            format!("pij ack dispatch-17 --packet-sha {}", "a".repeat(64))
        );

        let mut acknowledged = dispatches[0].clone();
        acknowledged.acknowledged = true;
        let acknowledged_rows = [acknowledged];
        let view = empty_view(&[], &[], &[], &acknowledged_rows, &[], &[]);
        assert!(DeliveredUnackedDetector.scan(&view).is_empty());
    }

    #[test]
    fn unverified_done_requires_a_verification_later_than_the_done_claim() {
        let before = DoneFact {
            seat: SeatId::from("pij-coder"),
            assignment_id: Some("asg-9".to_string()),
            done_seq: Seq(70),
            verified_seq: Some(Seq(69)),
            verified_done_seq: Some(Seq(70)),
        };
        let facts = [before];
        let view = empty_view(&[], &[], &[], &[], &facts, &[]);
        let rows = UnverifiedDoneDetector.scan(&view);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].remediation_line,
            "pij report verify pij-coder --assignment asg-9"
        );
        assert!(rows[0].remediation_line.contains("pij-coder"));

        let verified = [DoneFact {
            verified_seq: Some(Seq(71)),
            ..facts[0].clone()
        }];
        let view = empty_view(&[], &[], &[], &[], &verified, &[]);
        assert!(UnverifiedDoneDetector.scan(&view).is_empty());
    }

    #[test]
    fn cleared_occurrence_stays_cleared_until_source_evidence_changes() {
        let seats = [seat("pij-coder", None)];
        let actor = SeatId::from("pij-pm");
        let cards = [card("pij-coder", NOW - 700_000, 80)];
        let initial = empty_view(&seats, &cards, &[], &[], &[], &[]);
        let row = StatusStaleDetector.scan(&initial).remove(0);

        let ack = acknowledge_event(&row, &actor, NOW);
        let ack_events = [ack];
        let acknowledged = empty_view(&seats, &cards, &[], &[], &[], &ack_events);
        assert_eq!(
            StatusStaleDetector.scan(&acknowledged)[0].status,
            AnomalyStatus::Acknowledged
        );

        let clear = clear_event(&row, &actor, NOW + 1);
        let dispositions = [ack_events[0].clone(), clear];
        let cleared = empty_view(&seats, &cards, &[], &[], &[], &dispositions);
        assert!(StatusStaleDetector.scan(&cleared).is_empty());
        assert!(
            StatusStaleDetector.scan(&cleared).is_empty(),
            "another scan is not a recurrence"
        );

        let changed_cards = [card("pij-coder", NOW - 700_000, 81)];
        let recurred = empty_view(&seats, &changed_cards, &[], &[], &[], &dispositions);
        let rows = StatusStaleDetector.scan(&recurred);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, AnomalyStatus::Open);
        assert_ne!(rows[0].occurrence, row.occurrence);
    }

    #[test]
    fn disposition_events_are_appendable_spine_facts_for_the_exact_occurrence() {
        let fact = DoneFact {
            seat: SeatId::from("pij-coder"),
            assignment_id: Some("asg-10".to_string()),
            done_seq: Seq(90),
            verified_seq: None,
            verified_done_seq: None,
        };
        let facts = [fact];
        let view = empty_view(&[], &[], &[], &[], &facts, &[]);
        let row = UnverifiedDoneDetector.scan(&view).remove(0);
        let event = clear_event(&row, &SeatId::from("pij-pm"), NOW);

        assert_eq!(event.kind, ANOMALY_CLEAR_KIND);
        assert_eq!(event.seat, Some(SeatId::from("pij-coder")));
        let payload: DispositionPayload = serde_json::from_str(&event.payload).expect("payload");
        assert_eq!(payload.occurrence, row.occurrence);
        assert_eq!(payload.actor, SeatId::from("pij-pm"));
    }

    #[test]
    fn every_emitted_row_has_an_observable_and_runnable_remediation() {
        let seats = [seat("pij-card", None)];
        let cards = [card("pij-card", NOW - 700_000, 101)];
        let dispatches = [DispatchFact {
            seat: SeatId::from("pij-dispatch"),
            dispatch_id: "dispatch-22".to_string(),
            packet_sha256: "b".repeat(64),
            delivered_at: NOW - 1_000_000,
            delivered_seq: Seq(102),
            acknowledged: false,
        }];
        let done = [DoneFact {
            seat: SeatId::from("pij-done"),
            assignment_id: Some("asg-22".to_string()),
            done_seq: Seq(103),
            verified_seq: None,
            verified_done_seq: None,
        }];
        let view = empty_view(&seats, &cards, &[], &dispatches, &done, &[]);

        let rows = scan_all(&view);
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert!(!row.observable.trim().is_empty(), "{} observable", row.kind);
            assert!(
                row.remediation_line.starts_with("pij "),
                "{} remediation is runnable: {}",
                row.kind,
                row.remediation_line
            );
            if row.kind != AnomalyKind::DeliveredUnackedStale {
                assert!(
                    row.remediation_line.contains(row.seat.as_str()),
                    "{} remediation names its seat",
                    row.kind
                );
            }
        }
    }

    #[test]
    fn verification_of_another_done_does_not_close_latest_claim() {
        let done = [DoneFact {
            seat: SeatId::from("pij-worker"),
            assignment_id: None,
            done_seq: Seq(102),
            verified_seq: Some(Seq(103)),
            verified_done_seq: Some(Seq(100)),
        }];
        let view = empty_view(&[], &[], &[], &[], &done, &[]);
        let rows = UnverifiedDoneDetector.scan(&view);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].remediation_line, "pij report verify pij-worker");
        assert!(rows[0].wire().assignment_id.is_none());
    }

    #[test]
    fn open_decision_stales_responsible_parent_despite_fresh_card() {
        let parent = seat("pij-parent", None);
        let mut worker = seat("pij-worker", None);
        worker.parent = Some(parent.id.clone());
        let seats = [parent, worker];
        let cards = [card("pij-parent", NOW, 105)];
        let decisions = [crate::decisions::Decision {
            id: "d1".to_string(),
            asked_by: SeatId::from("pij-worker"),
            parent: Some(SeatId::from("pij-parent")),
            question: "which branch?".to_string(),
            state: crate::decisions::DecisionState::Open,
            asked_at: NOW - 1_800_001,
            question_seq: Seq(100),
            answer: None,
            answered_by: None,
            answered_at: None,
            answer_msg_id: None,
        }];
        let mut view = empty_view(&seats, &cards, &[], &[], &[], &[]);
        view.decisions = &decisions;
        let rows = scan_all(&view);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.kind == AnomalyKind::OpenQuestion));
        let stale = rows
            .iter()
            .find(|row| row.kind == AnomalyKind::StatusStale)
            .expect("overdue parent");
        assert_eq!(stale.seat.as_str(), "pij-parent");
        assert_eq!(stale.wire().record_ref, Some("decision:d1"));
        assert_eq!(stale.wire().evidence, vec![Seq(100)]);
    }

    #[test]
    fn five_governance_kinds_match_canonical_rows_and_real_sequences() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contract");
        let seats = [seat("pij-worker", None)];
        let cards = [card("pij-worker", NOW - 1_800_001, 100)];
        let dispatches = [DispatchFact {
            seat: SeatId::from("pij-worker"),
            dispatch_id: "dispatch-139-proof".into(),
            packet_sha256: "a".repeat(64),
            delivered_at: NOW - 900_001,
            delivered_seq: Seq(100),
            acknowledged: false,
        }];
        let done = [DoneFact {
            seat: SeatId::from("pij-worker"),
            assignment_id: Some("assignment-139-proof".into()),
            done_seq: Seq(100),
            verified_seq: None,
            verified_done_seq: None,
        }];
        let mut decision: Decision =
            serde_json::from_value(fixtures["rows"]["decision"].clone()).expect("decision");
        decision.asked_at = NOW - 1;
        let decisions = [decision];
        let dead = [DeadSeatFact {
            seat: SeatId::from("pij-worker"),
            seq: Seq(100),
        }];
        let mut view = empty_view(&seats, &cards, &[], &dispatches, &done, &[]);
        view.thresholds = AnomalyThresholds::default();
        view.decisions = &decisions;
        view.dead = &dead;
        let rows = scan_all(&view);
        assert_eq!(rows.len(), 5);
        for expected in fixtures["rows"]["anomalies"].as_array().expect("rows") {
            let row = rows
                .iter()
                .find(|row| row.kind.as_str() == expected["kind"].as_str().expect("kind"))
                .expect("existing kind spelling");
            assert_eq!(serde_json::to_value(row.wire()).expect("wire"), *expected);
        }
    }
}
