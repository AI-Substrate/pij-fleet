//! Status-card and declared-state service.
//!
//! Cards and state history share the append-only [`Spine`]. The [`Registry`]
//! remains authoritative for the current semantic state: state changes persist
//! there first, then append their note and audit record. The two frozen ports do
//! not offer a cross-port transaction, so a history failure reports the true
//! partial result instead of claiming the state change rolled back.

use serde::{Deserialize, Serialize};

use crate::error::{PijError, Result};
use crate::model::{CARD_LIMIT, Card, Event, SeatId, SemanticState, Seq};
use crate::ports::{Registry, Spine};

/// Spine kind for a now/next card.
pub const CARD_EVENT_KIND: &str = "report.now";
/// Spine kind for a semantic-state declaration or clear.
pub const STATE_EVENT_KIND: &str = "report.state";

/// Runtime report policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportConfig {
    /// A card is stale only when its age is strictly greater than this value.
    pub stale_after_ms: u64,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            stale_after_ms: 10 * 60 * 1_000,
        }
    }
}

/// The durable payload of a card event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardRecord {
    /// What the seat just finished, after whitespace collapsing.
    pub did: String,
    /// What the seat intends to do next, after whitespace collapsing.
    pub next: String,
}

/// The durable payload of a declared-state event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRecord {
    /// The new state, or `None` when the declaration was cleared.
    pub state: Option<SemanticState>,
    /// The explanation supplied with `blocked` or `question`.
    pub note: Option<String>,
    /// The task this declaration concerns, validated by the caller.
    #[serde(default)]
    pub assignment_id: Option<String>,
    /// Supporting references; these may accompany an unscoped declaration.
    #[serde(default)]
    pub refs: Vec<String>,
    /// The authoritative registry write this history record describes.
    pub registry_seq: Seq,
}

/// A card together with freshness computed at read time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardStatus {
    /// The durable card.
    pub card: Card,
    /// Age at the injected clock reading. Backwards clock movement saturates at zero.
    pub age_ms: u64,
    /// Whether `age_ms` is strictly greater than the configured threshold.
    pub stale: bool,
}

/// Report operations over the two existing persistence ports and an injected clock.
pub struct ReportService<'a, R: ?Sized, S: ?Sized, C> {
    registry: &'a R,
    spine: &'a S,
    clock: C,
    config: ReportConfig,
}

impl<'a, R, S, C> ReportService<'a, R, S, C>
where
    R: Registry + ?Sized,
    S: Spine + ?Sized,
    C: Fn() -> u64,
{
    /// Build a report service. `clock` returns Unix-epoch milliseconds.
    ///
    /// # Composition recipe
    ///
    /// The composition root must expose its EventBus as `services.spine`, not
    /// the raw store: report writes then share registry ordering and live fanout.
    ///
    /// ```ignore
    /// use pij_core::report::{ReportConfig, ReportService};
    /// let reports = ReportService::new(
    ///     services.registry.as_ref(),
    ///     services.spine.as_ref(),
    ///     clock,
    ///     ReportConfig { stale_after_ms: configured_report_stale_ms },
    /// );
    /// ```
    pub fn new(registry: &'a R, spine: &'a S, clock: C, config: ReportConfig) -> Self {
        Self {
            registry,
            spine,
            clock,
            config,
        }
    }

    /// Persist a now/next card and return its spine sequence.
    ///
    /// Each field is limited independently after whitespace collapsing. Empty is
    /// deliberately valid: a present empty card and no card are different facts.
    pub async fn now(&self, seat: &SeatId, did: &str, next: &str) -> Result<Seq> {
        let record = CardRecord {
            did: normalize_card_field(did)?,
            next: normalize_card_field(next)?,
        };
        let at = (self.clock)();
        self.spine
            .append(Event {
                seq: None,
                v: 1,
                at,
                kind: CARD_EVENT_KIND.to_string(),
                seat: Some(seat.clone()),
                payload: encode(&record)?,
            })
            .await
    }

    /// Read the latest card and compute its age and staleness.
    ///
    /// Semantic state is intentionally not consulted. Parking suspends watchdog
    /// nudging, not truthful display of an old card. Equality with the threshold
    /// is fresh; only a strictly older card is stale.
    pub async fn card(&self, seat: &SeatId) -> Result<Option<CardStatus>> {
        let Some(event) = self.latest_event(seat, CARD_EVENT_KIND).await? else {
            return Ok(None);
        };
        let record: CardRecord = decode(&event.payload, CARD_EVENT_KIND)?;
        let seq = event.seq.ok_or_else(|| PijError::Adapter {
            adapter: "report".to_string(),
            message: format!("{CARD_EVENT_KIND} read from Spine had no sequence"),
        })?;
        let age_ms = (self.clock)().saturating_sub(event.at);
        Ok(Some(CardStatus {
            card: Card {
                seat: seat.clone(),
                did: record.did,
                next: record.next,
                at: event.at,
                seq: Some(seq),
            },
            age_ms,
            stale: age_ms > self.config.stale_after_ms,
        }))
    }

    /// Declare work finished.
    pub async fn done(&self, seat: &SeatId) -> Result<Seq> {
        self.set_state(seat, Some(SemanticState::Done), None, None, Vec::new())
            .await
    }

    /// Declare a blocking dependency and retain its explanation in the spine.
    pub async fn blocked(&self, seat: &SeatId, note: &str) -> Result<Seq> {
        self.set_state(
            seat,
            Some(SemanticState::Blocked),
            Some(collapse_whitespace(note)),
            None,
            Vec::new(),
        )
        .await
    }

    /// Declare a human question and retain it in the spine.
    pub async fn question(&self, seat: &SeatId, note: &str) -> Result<Seq> {
        self.set_state(
            seat,
            Some(SemanticState::Question),
            Some(collapse_whitespace(note)),
            None,
            Vec::new(),
        )
        .await
    }

    /// Clear the current semantic-state declaration.
    pub async fn clear(&self, seat: &SeatId) -> Result<Seq> {
        self.set_state(seat, None, None, None, Vec::new()).await
    }

    /// Declare (or clear) a semantic state directly.
    ///
    /// [`done`](Self::done), [`blocked`](Self::blocked), [`question`](Self::question)
    /// and [`clear`](Self::clear) are the four declarations this service had, and
    /// they cover four of the six words [`SemanticState`] carries: `Ready`,
    /// `Waiting` and `Hold` had no way in. A caller reaching for one of those had
    /// only two options — re-implement the registry-then-spine sequence beside
    /// this service, or map the state onto a near-fit that IS exposed. Both are
    /// how the two would drift, so the sequence is exposed once, here.
    ///
    /// `note` is retained exactly as the explaining declarations retain theirs;
    /// `None` for `state` clears, which is what [`clear`](Self::clear) is.
    pub async fn declare(
        &self,
        seat: &SeatId,
        state: Option<SemanticState>,
        note: Option<&str>,
        assignment_id: Option<&str>,
        refs: &[String],
    ) -> Result<Seq> {
        self.set_state(
            seat,
            state,
            note.map(collapse_whitespace),
            assignment_id.map(str::to_string),
            refs.to_vec(),
        )
        .await
    }

    /// Read the latest state-history record, independently of the registry row.
    pub async fn latest_state_record(&self, seat: &SeatId) -> Result<Option<StateRecord>> {
        let Some(event) = self.latest_event(seat, STATE_EVENT_KIND).await? else {
            return Ok(None);
        };
        decode(&event.payload, STATE_EVENT_KIND).map(Some)
    }

    async fn set_state(
        &self,
        seat: &SeatId,
        state: Option<SemanticState>,
        note: Option<String>,
        assignment_id: Option<String>,
        refs: Vec<String>,
    ) -> Result<Seq> {
        let mut descriptor =
            self.registry
                .get(seat)
                .await?
                .ok_or_else(|| PijError::NoRegistryEntry {
                    seat: seat.clone(),
                    // This layer consulted the REGISTRY PORT, not a file: it names what
                    // it actually searched at the granularity it knows, and the
                    // adapter names the path when the adapter is the one that looked.
                    store: "the daemon registry".to_string(),
                })?;
        descriptor.semantic_state = state;
        let registry_seq = self.registry.put(descriptor).await?;
        let record = StateRecord {
            state,
            note,
            assignment_id,
            refs,
            registry_seq,
        };
        let append = self
            .spine
            .append(Event {
                seq: None,
                v: 1,
                at: (self.clock)(),
                kind: STATE_EVENT_KIND.to_string(),
                seat: Some(seat.clone()),
                payload: encode(&record)?,
            })
            .await;
        append.map_err(|error| PijError::Adapter {
            adapter: "report".to_string(),
            message: format!(
                "{seat}: semantic state changed in the Registry at sequence {}; its note and state history were not recorded in the Spine: {error}",
                registry_seq.0
            ),
        })
    }

    async fn latest_event(&self, seat: &SeatId, kind: &str) -> Result<Option<Event>> {
        Ok(self
            .spine
            .tail(Some(seat), Seq(0))
            .await?
            .into_iter()
            .rev()
            .find(|event| event.kind == kind))
    }
}

fn normalize_card_field(input: &str) -> Result<String> {
    let normalized = collapse_whitespace(input);
    let len = normalized.chars().count();
    if len > CARD_LIMIT {
        return Err(PijError::ReportTooLong {
            len,
            limit: CARD_LIMIT,
        });
    }
    Ok(normalized)
}

fn collapse_whitespace(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    for word in input.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    normalized
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| PijError::Adapter {
        adapter: "report".to_string(),
        message: format!("could not encode report event: {error}"),
    })
}

fn decode<T: for<'de> Deserialize<'de>>(payload: &str, kind: &str) -> Result<T> {
    serde_json::from_str(payload).map_err(|error| PijError::Adapter {
        adapter: "report".to_string(),
        message: format!("could not decode {kind} payload: {error}"),
    })
}
