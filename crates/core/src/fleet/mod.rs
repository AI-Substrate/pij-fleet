//! The fleet report (plan 162): what a project's agent fleet cost, and why.
//!
//! `pij fleet-report` joins two kinds of facts. Unisphere reads the transcripts
//! (calls, turns, events) and pij adds what only it knows (seats and roles). This
//! module is the pure half: it takes both as plain rows and computes every
//! aggregate the report page draws. It does no IO and knows no transcript format.
//!
//! **Prices are the reader's.** Linear aggregates are carried as [`Tokens`], one
//! count per token class, so the page can price them at list price or with
//! cached reads free, and a price change needs no re-run. Statistics that are not
//! linear in the prices (medians, growth exponents) are computed here at the
//! report's own price table and say so.
//!
//! The rules follow the 2026-09 usage-blowout RCA pipeline, so the report
//! reproduces its figures.

mod analysis;
mod civil;
mod growth;
mod report;
mod status;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use analysis::analyze;
pub use civil::{bucket_key, day_key};
pub use report::*;

/// The shape version of `report.json`. Bump it when a field changes meaning.
pub const REPORT_VERSION: u32 = 1;

/// A call that wrote at least half its context, above this size, rebuilt its cache.
pub const COLD_MIN_CONTEXT: u64 = 20_000;

/// The prompt-cache lifetime Claude Code requests for a main session.
pub const TTL_MAIN_MS: i64 = 3_600_000;

/// The prompt-cache lifetime of a subagent's cache.
pub const TTL_SUB_MS: i64 = 300_000;

/// A message-opened turn of at most this many calls needed no action: a status turn.
pub const STATUS_TURN_MAX_CALLS: usize = 3;

/// Token counts by price class. Every linear aggregate in the report is one of these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// Uncached input.
    pub input: u64,
    /// Cache writes with the 1-hour lifetime.
    pub cw_1h: u64,
    /// Cache writes with the 5-minute lifetime.
    pub cw_5m: u64,
    /// Cached reads.
    pub cache_read: u64,
    /// Output, including hidden thinking.
    pub output: u64,
}

impl Tokens {
    /// The context a call sent: everything but its output.
    pub fn context(&self) -> u64 {
        self.input + self.cw_1h + self.cw_5m + self.cache_read
    }

    /// Cache writes of both lifetimes.
    pub fn writes(&self) -> u64 {
        self.cw_1h + self.cw_5m
    }

    /// Add another count in place.
    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.cw_1h += other.cw_1h;
        self.cw_5m += other.cw_5m;
        self.cache_read += other.cache_read;
        self.output += other.output;
    }

    /// USD at `prices`.
    pub fn usd(&self, prices: &Prices) -> f64 {
        (self.input as f64 * prices.input
            + self.cw_1h as f64 * prices.write_1h
            + self.cw_5m as f64 * prices.write_5m
            + self.cache_read as f64 * prices.read
            + self.output as f64 * prices.output)
            / 1e6
    }

    /// USD at `prices` with cached reads free.
    pub fn usd_reads_free(&self, prices: &Prices) -> f64 {
        self.usd(prices) - self.cache_read as f64 * prices.read / 1e6
    }
}

/// USD per million tokens, by class, for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    /// Uncached input.
    pub input: f64,
    /// 1-hour cache writes.
    pub write_1h: f64,
    /// 5-minute cache writes.
    pub write_5m: f64,
    /// Cached reads.
    pub read: f64,
    /// Output.
    pub output: f64,
    /// The model's context window, in tokens.
    pub window: u64,
}

impl Prices {
    /// Claude Opus 5.5 API list prices, the RCA's yardstick.
    pub fn opus_5_5() -> Self {
        Self {
            input: 4.0,
            write_1h: 8.0,
            write_5m: 5.0,
            read: 0.2,
            output: 20.0,
            window: 1_000_000,
        }
    }

    /// Claude Sonnet 5.5 API list prices.
    pub fn sonnet_5_5() -> Self {
        Self {
            input: 2.0,
            write_1h: 4.0,
            write_5m: 2.5,
            read: 0.2,
            output: 10.0,
            window: 1_000_000,
        }
    }

    /// Claude Haiku 4.5 API list prices.
    pub fn haiku_4_5() -> Self {
        Self {
            input: 1.0,
            write_1h: 2.0,
            write_5m: 1.25,
            read: 0.1,
            output: 5.0,
            window: 200_000,
        }
    }
}

/// One deduplicated API call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// The transcript the call came from (Unisphere's source key).
    pub source: String,
    /// A subagent's transcript, whose cache lives 5 minutes.
    pub is_sub: bool,
    /// When, in UTC milliseconds.
    pub ts_ms: i64,
    /// The model, when recorded.
    pub model: Option<String>,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Milliseconds since the previous call in the same transcript; `None` for the first.
    pub gap_ms: Option<i64>,
    /// The turn this call belongs to; 0 before the first opener.
    pub turn_no: i64,
    /// Position in its turn, from 1.
    pub call_in_turn: i64,
}

impl Call {
    /// The context it sent.
    pub fn context(&self) -> u64 {
        self.tokens.context()
    }

    /// Recorded cold: the call wrote at least half its context, so the cached prefix was gone.
    pub fn cold(&self) -> bool {
        let context = self.context();
        context > COLD_MIN_CONTEXT && self.tokens.writes() * 2 >= context
    }

    /// The cache's lifetime for this call's transcript.
    pub fn ttl_ms(&self) -> i64 {
        if self.is_sub { TTL_SUB_MS } else { TTL_MAIN_MS }
    }

    /// The idle gap before this call outlived the cache.
    pub fn idle(&self) -> bool {
        self.gap_ms.is_some_and(|gap| gap > self.ttl_ms())
    }
}

/// What opened a turn, in Unisphere's vocabulary (`peer`, `human`, `start`, …).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// The transcript (Unisphere's source key).
    pub source: String,
    /// The turn number in its transcript.
    pub turn_no: i64,
    /// What opened it.
    pub origin: String,
    /// The sending seat of a peer-opened turn.
    pub sender: Option<String>,
    /// The pij message id the opener carried.
    pub pij_msg_id: Option<String>,
    /// When the turn opened.
    pub started_ms: Option<i64>,
    /// The opener's first words; only with `--include-content`.
    pub head: Option<String>,
}

/// A transcript event that is not a call: a compaction, recap, limit notice, ….
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The transcript (Unisphere's source key).
    pub source: String,
    /// When, in UTC milliseconds.
    pub ts_ms: Option<i64>,
    /// Unisphere's event kind (`compaction`, `recap`, `limit_notice`, `model_switch`, …).
    pub kind: String,
    /// The harness-specific subkind.
    pub subkind: Option<String>,
    /// Compaction trigger (`manual`/`auto`).
    pub trigger: Option<String>,
    /// The model, when recorded.
    pub model: Option<String>,
    /// Compaction: tokens before.
    pub pre_tokens: Option<i64>,
    /// Compaction: tokens after (native only).
    pub post_tokens: Option<i64>,
    /// How long it took.
    pub duration_ms: Option<i64>,
    /// The context of the last call before it.
    pub last_context: Option<i64>,
    /// Milliseconds since the last call; `None` when there was none.
    pub gap_ms: Option<i64>,
    /// Limit notice: the reset it named.
    pub resets_at: Option<String>,
}

/// One transcript in scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The transcript (Unisphere's source key).
    pub source: String,
    /// Unisphere's harness id (`claude-code`, `oh-my-pi`, …).
    pub harness: String,
    /// The harness session id.
    pub session_id: Option<String>,
    /// A subagent's parent session.
    pub parent_session_id: Option<String>,
    /// A subagent's transcript.
    pub is_sub: bool,
    /// The working directory the harness recorded.
    pub cwd: Option<String>,
    /// First event.
    pub first_ms: Option<i64>,
    /// Last event.
    pub last_ms: Option<i64>,
}

/// One pij seat incarnation, from pij's store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    /// The seat id.
    pub id: String,
    /// The harness.
    pub harness: String,
    /// The asserted role.
    pub role: Option<String>,
    /// The registered folder.
    pub folder: String,
    /// The governing seat.
    pub parent: Option<String>,
    /// First registration.
    pub spawned_ms: Option<i64>,
    /// Tombstone.
    pub ended_ms: Option<i64>,
    /// Every harness session id the seat has been bound to.
    pub sessions: Vec<String>,
}

/// Everything one report is computed from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Corpus {
    /// Transcripts in scope.
    pub sessions: Vec<Session>,
    /// Their calls.
    pub calls: Vec<Call>,
    /// Their turns.
    pub turns: Vec<Turn>,
    /// Their events.
    pub events: Vec<Event>,
    /// pij's seats.
    pub seats: Vec<Seat>,
}

/// The report's window and the clock its buckets are read on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// Start (inclusive), UTC milliseconds.
    pub since_ms: i64,
    /// End (exclusive), UTC milliseconds.
    pub until_ms: i64,
    /// Minutes east of UTC for hour and day buckets.
    pub utc_offset_min: i32,
}

impl Window {
    /// Is `ts_ms` inside the window?
    pub fn contains(&self, ts_ms: i64) -> bool {
        self.since_ms <= ts_ms && ts_ms < self.until_ms
    }
}
