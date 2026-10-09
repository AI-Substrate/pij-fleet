//! What a PA's watchdog nudge carries: its prime's fleet at a glance.
//!
//! A PA is the fleet's watchdog and context keeper. Its nudge brings the
//! fleet's sizes, coldness and attention flags with it, so the PA acts on the
//! nudge without a second `pij list` call. Pure: the daemon assembles the
//! rows; this module decides the quiet fingerprint and the text. Policy (caps,
//! exemptions, when to compact) lives in the PA brief, never here — the digest
//! reports facts and flags states, it does not tell the PA what to do.

use std::fmt::Write;

use crate::cold_wake::{SeatSize, human_duration, human_tokens};
use crate::model::{SeatDescriptor, SeatId, SemanticState, SystemState};
use crate::session_status::CacheState;

/// One seat in the PA's fleet, with its size facts on the daemon clock.
#[derive(Clone, Debug, PartialEq)]
pub struct FleetRow {
    /// Registry descriptor, role already joined from the role store.
    pub seat: SeatDescriptor,
    /// Context, idle time, cache state and cold-wake verdict.
    pub size: SeatSize,
}

/// An open anomaly on a seat in the fleet (`pij anomalies`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetAnomaly {
    /// Seat the anomaly concerns.
    pub seat: SeatId,
    /// Stable kind, e.g. `status-stale`.
    pub kind: String,
    /// Human detail.
    pub detail: String,
}

/// A PA's fleet: every live seat sharing its prime's repository.
#[derive(Clone, Debug, PartialEq)]
pub struct FleetView {
    /// The PA being nudged. Its own row is listed but never counts as change.
    pub pa: SeatId,
    /// The PA's recorded parent, when it has one.
    pub prime: Option<SeatId>,
    /// Display name of the scope: the repository's common git dir or folder.
    pub scope: String,
    /// Live seats in scope.
    pub rows: Vec<FleetRow>,
    /// Open anomalies on seats in scope.
    pub anomalies: Vec<FleetAnomaly>,
    /// The watchdog interval, which also defines a ready seat as overdue.
    pub interval_ms: u64,
}

/// Why a seat is in the "needs a look" list.
///
/// Two kinds of flag. A seat the old per-seat watchdog would have nudged
/// ([`SeatDescriptor::nudgeable`]) that has been quiet a whole interval; and a
/// seat parked on purpose (`waiting`, `hold`, `blocked`, `question`), which the
/// PA should know about but which is not a stall. The PA decides what, if
/// anything, to send; the digest only names the state.
fn attention(row: &FleetRow, interval_ms: u64) -> Option<String> {
    if row.seat.nudgeable() {
        return row
            .size
            .idle_ms
            .filter(|&idle| idle >= interval_ms)
            .map(|idle| {
                let label = row
                    .seat
                    .semantic_state
                    .map_or("idle", SemanticState::as_str);
                format!("{label}, quiet {}", human_duration(idle))
            });
    }
    match (row.seat.state, row.seat.semantic_state) {
        (SystemState::Working, _) => None,
        (
            _,
            Some(
                state @ (SemanticState::Waiting
                | SemanticState::Hold
                | SemanticState::Blocked
                | SemanticState::Question),
            ),
        ) => Some(match row.size.idle_ms {
            Some(idle) => format!("{}, quiet {}", state.as_str(), human_duration(idle)),
            None => state.as_str().to_string(),
        }),
        _ => None,
    }
}

impl FleetView {
    /// What must differ between two rounds for the PA to be nudged again.
    ///
    /// Turn state, declared state, context size, attention flags and open
    /// anomalies of every seat except the PA itself. Idle time and cache
    /// warmth are excluded: they change on every read without anything
    /// happening. The PA's own row is excluded because answering one nudge
    /// would otherwise manufacture the change that causes the next.
    pub fn fingerprint(&self) -> String {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .filter(|row| row.seat.id != self.pa)
            .map(|row| {
                format!(
                    "{}|{}|{}|{}|{}",
                    row.seat.id,
                    row.seat.state.as_str(),
                    row.seat.semantic_state.map_or("-", SemanticState::as_str),
                    row.size
                        .context_used
                        .map_or_else(|| "?".to_string(), |tokens| tokens.to_string()),
                    attention(row, self.interval_ms).is_some(),
                )
            })
            .collect();
        lines.extend(
            self.anomalies
                .iter()
                .filter(|anomaly| anomaly.seat != self.pa)
                .map(|anomaly| format!("!{}|{}", anomaly.seat, anomaly.kind)),
        );
        lines.sort();
        lines.join("\n")
    }

    /// The nudge text: a headline, what needs a look, and every seat by size.
    pub fn digest(&self) -> String {
        let working = self
            .rows
            .iter()
            .filter(|row| row.seat.state == SystemState::Working)
            .count();
        let cold = self
            .rows
            .iter()
            .filter(|row| row.size.cold_wake.would_refuse)
            .count();
        let mut text = format!(
            "[pij watchdog] fleet round for {} ({}) — {} seats: {working} working, {} idle, {cold} cold ❄",
            self.prime.as_ref().map_or("your fleet", SeatId::as_str),
            self.scope,
            self.rows.len(),
            self.rows.len() - working,
        );

        let mut flagged: Vec<(String, String)> = self
            .rows
            .iter()
            .filter(|row| row.seat.id != self.pa)
            .filter_map(|row| {
                attention(row, self.interval_ms).map(|why| (row.seat.id.to_string(), why))
            })
            .collect();
        flagged.extend(self.anomalies.iter().map(|anomaly| {
            (
                anomaly.seat.to_string(),
                format!("{}: {}", anomaly.kind, anomaly.detail),
            )
        }));
        flagged.sort();
        text.push_str("\nNeeds a look:");
        if flagged.is_empty() {
            text.push_str(" nothing flagged");
        }
        for (seat, why) in &flagged {
            let _ = write!(text, "\n  {seat}  {why}");
        }

        let mut rows: Vec<&FleetRow> = self.rows.iter().collect();
        rows.sort_by(|a, b| {
            b.size
                .context_used
                .cmp(&a.size.context_used)
                .then_with(|| a.seat.id.cmp(&b.seat.id))
        });
        let width = rows
            .iter()
            .map(|row| row.seat.id.as_str().len())
            .max()
            .unwrap_or(4)
            .max(4);
        let _ = write!(
            text,
            "\nSeats by context:\n  {:width$}  {:6}  {:7}  {:8}  {:>5}  {:>6}  cache",
            "seat", "role", "state", "declared", "ctx", "idle"
        );
        for row in rows {
            let you = if row.seat.id == self.pa { " (you)" } else { "" };
            let cache = match &row.size.cache_state {
                Some(CacheState::Warm { .. }) => "warm".to_string(),
                Some(CacheState::Cold { expired_for_ms }) => {
                    format!("cold {}", human_duration(*expired_for_ms))
                }
                None => "?".to_string(),
            };
            let wake = if row.size.cold_wake.would_refuse {
                row.size
                    .cold_wake
                    .estimate_usd
                    .map_or_else(|| " ❄".to_string(), |usd| format!(" ❄ ${usd:.2} to wake"))
            } else {
                String::new()
            };
            let _ = write!(
                text,
                "\n  {:width$}  {:6}  {:7}  {:8}  {:>5}  {:>6}  {cache}{wake}{you}",
                row.seat.id.as_str(),
                row.seat.role.as_deref().unwrap_or("-"),
                row.seat.state.as_str(),
                row.seat.semantic_state.map_or("-", SemanticState::as_str),
                row.size
                    .context_used
                    .map_or_else(|| "?".to_string(), human_tokens),
                row.size
                    .idle_ms
                    .map_or_else(|| "?".to_string(), human_duration),
            );
        }
        text.push_str(
            "\nThis nudge is your clock: act per your PA brief, then end your turn. \
             Set no timers or background loops; pij nudges you again when the fleet changes.",
        );
        text
    }
}
