//! The shape of `report.json`: everything the page draws.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Prices, Tokens, Window};

/// The price table the report carries. The page prices [`Tokens`] with it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PriceTable {
    /// The model aggregates are priced at by default (and nonlinear statistics were computed at).
    pub default: String,
    /// Prices by model name.
    pub models: BTreeMap<String, Prices>,
}

impl Default for PriceTable {
    fn default() -> Self {
        let models = BTreeMap::from([
            ("opus-5.5".to_string(), Prices::opus_5_5()),
            ("sonnet-5.5".to_string(), Prices::sonnet_5_5()),
            ("haiku-4.5".to_string(), Prices::haiku_4_5()),
        ]);
        Self {
            default: "opus-5.5".to_string(),
            models,
        }
    }
}

impl PriceTable {
    /// The default model's prices.
    pub fn base(&self) -> Prices {
        self.models
            .get(&self.default)
            .cloned()
            .unwrap_or_else(Prices::opus_5_5)
    }
}

/// The whole report.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// The report shape ([`super::REPORT_VERSION`]).
    pub version: u32,
    /// The window the aggregates cover.
    pub window: Option<Window>,
    /// What the report covers, filled in by the writer.
    pub scope: Option<Scope>,
    /// The price table to price [`Tokens`] with.
    pub prices: PriceTable,
    /// Counts over the window.
    pub totals: Totals,
    /// The spend split.
    pub components: Components,
    /// Hourly buckets over the window.
    pub hourly: Vec<Bucket>,
    /// Daily buckets over the window.
    pub daily: Vec<Bucket>,
    /// Ten-minute token totals, for the meter (cumulative spend from any start).
    pub series_10m: Vec<SeriesPoint>,
    /// Spend by what opened the turn.
    pub triggers: Vec<TriggerRow>,
    /// Who woke whom: peer-opened turns by sender.
    pub senders: Vec<SenderRow>,
    /// The message graph.
    pub graph: Graph,
    /// Context lanes of the costliest main sessions.
    pub lanes: Vec<Lane>,
    /// Compactions in the window.
    pub compactions: CompactionSummary,
    /// Usage-limit notices the harness wrote.
    pub limits: Vec<LimitNotice>,
    /// How cost scales with context.
    pub context_cost: ContextCost,
    /// Which costs follow the square rule.
    pub square_rule: SquareRule,
    /// What no-action messages cost.
    pub status_turns: StatusTurns,
    /// Cold calls, idle and rebuild.
    pub cold_wakes: ColdWakes,
    /// What consulting a cold seat costs, by model.
    pub consult: Consult,
    /// List-price headline figures, for prose and for checking against a reference.
    pub key_figures: KeyFigures,
}

/// The folders a report covers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Scope {
    /// The project root; `None` when anonymised (a path names a project).
    pub folder: Option<String>,
    /// Folders in scope, the root and its worktrees.
    pub folders: u64,
}

/// Counts over the window.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Totals {
    /// Calls.
    pub calls: u64,
    /// Turns.
    pub turns: u64,
    /// Sessions with a call in the window.
    pub sessions: u64,
    /// Of those, main (not subagent) sessions.
    pub main_sessions: u64,
    /// Main sessions bound to a pij seat; the rest are `unseated` (started by hand).
    pub seated_sessions: u64,
    /// Main sessions with no pij seat.
    pub unseated_sessions: u64,
    /// Distinct seats among the sessions.
    pub seats: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Calls that rebuilt their cache.
    pub cold_calls: u64,
    /// Mean context per call.
    pub mean_context: u64,
}

/// The spend split: mutually exclusive parts of every call, plus simulated hidden requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Components {
    /// Cached reads of warm calls (every call re-reads the conversation).
    pub warm_reads: Tokens,
    /// Cache writes of cold calls: idle wakes after expiry, rebuilds after compaction.
    pub cold_rewrite: Tokens,
    /// Cache writes of warm calls: new tool results and replies.
    pub incremental_writes: Tokens,
    /// Output and uncached input.
    pub output_cost: Tokens,
    /// Compactions, which are not recorded as calls: simulated.
    pub compaction_sim: Tokens,
    /// Idle recaps (away summaries), simulated.
    pub recap_sim: Tokens,
}

/// One hour or day.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Bucket {
    /// `YYYY-MM-DDTHH` (hourly) or `YYYY-MM-DD` (daily), on the report's clock.
    pub key: String,
    /// Calls.
    pub calls: u64,
    /// Sessions with a call in the window.
    pub sessions: u64,
    /// Cold calls (or turns with one).
    pub cold: u64,
    /// The spend split.
    pub components: Components,
    /// Call tokens by what opened the turn.
    pub by_trigger: BTreeMap<String, Tokens>,
    /// Compactions.
    pub compactions: u64,
    /// Turns opened by a peer message.
    pub peer_turns: u64,
    /// Turns opened by the human.
    pub human_turns: u64,
}

/// One ten-minute bucket.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SeriesPoint {
    /// The bucket key.
    pub key: String,
    /// Tokens by class.
    pub tokens: Tokens,
}

/// Spend of one trigger group.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TriggerRow {
    /// The trigger group, or what opened the turn.
    pub trigger: String,
    /// Turns.
    pub turns: u64,
    /// Calls.
    pub calls: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Turns with a cold call.
    pub cold_wakes: u64,
    /// The cache writes of those cold calls.
    pub cold_tokens: Tokens,
    /// Median calls per turn.
    pub median_calls_per_turn: f64,
}

/// Turns one seat opened in others.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SenderRow {
    /// The sending seat.
    pub sender: String,
    /// Turns.
    pub turns: u64,
    /// Distinct recipient sessions.
    pub recipients: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Turns with a cold call.
    pub cold_wakes: u64,
    /// The cache writes of those cold calls.
    pub cold_tokens: Tokens,
    /// Turns of three calls or fewer.
    pub short_turns: u64,
}

/// Who messaged whom.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Graph {
    /// Seats and unseated sessions.
    pub nodes: Vec<GraphNode>,
    /// Peer-opened turns between them.
    pub edges: Vec<GraphEdge>,
    /// pij messages per pair touching a seat in scope (from pij's own records).
    pub messages: Vec<super::MessageCount>,
}

/// A seat, or an unseated session.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GraphNode {
    /// The seat id, or `session <id8>` for an unseated session.
    pub id: String,
    /// The seat's role, when pij knows it.
    pub role: Option<String>,
    /// The seat's harness, when pij knows it.
    pub harness: Option<String>,
    /// Bound to a pij seat.
    pub seated: bool,
    /// A project's prime (or the machine's designated prime): a hub.
    pub prime: bool,
    /// A seat of this machine's pij store; false for another machine's seat.
    pub local: bool,
    /// pij messages it sent in the window.
    pub sent: u64,
    /// pij messages it received in the window.
    pub received: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// The largest context.
    pub max_context: u64,
}

/// Peer-opened main-session turns from one seat to another.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GraphEdge {
    /// Sending seat.
    pub from: String,
    /// Receiving seat.
    pub to: String,
    /// Turns.
    pub turns: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Cold calls (or turns with one).
    pub cold: u64,
}

/// One main session's context over time.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Lane {
    /// The seat (or unseated session) label.
    pub seat: String,
    /// The seat's role, when pij knows it.
    pub role: Option<String>,
    /// Tokens by class.
    pub tokens: Tokens,
    /// Calls.
    pub calls: u64,
    /// Turns opened by a peer message.
    pub peer_turns: u64,
    /// Turns opened by the human.
    pub human_turns: u64,
    /// Five-minute buckets.
    pub points: Vec<LanePoint>,
    /// Its cold calls.
    pub cold: Vec<ColdWake>,
    /// Its compactions.
    pub compactions: Vec<LaneCompaction>,
}

/// Five-minute bucket of one lane.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LanePoint {
    /// The bucket key.
    pub key: String,
    /// The largest context.
    pub max_context: u64,
    /// Calls.
    pub calls: u64,
}

/// A compaction on a lane.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LaneCompaction {
    /// When, in UTC milliseconds.
    pub ts_ms: i64,
    /// Tokens before.
    pub pre: i64,
    /// Tokens after.
    pub post: i64,
}

/// One cold call.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ColdWake {
    /// When, in UTC milliseconds.
    pub ts_ms: i64,
    /// The seat (or unseated session) label.
    pub seat: String,
    /// The cache it rewrote.
    pub tokens: Tokens,
    /// Idle minutes before it.
    pub gap_min: f64,
    /// The trigger group, or what opened the turn.
    pub trigger: String,
    /// The sending seat of a peer-opened turn.
    pub sender: Option<String>,
    /// `idle` (the cache expired) or `rebuild` (after a compaction or restart).
    pub kind: String,
    /// Opened by a status turn: avoidable.
    pub status: bool,
}

/// Compactions in the window.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CompactionSummary {
    /// Compactions.
    pub count: u64,
    /// Of those, manual.
    pub manual: u64,
    /// Cold calls (or turns with one).
    pub cold: u64,
    /// Median tokens before.
    pub median_pre: i64,
    /// Total tokens before.
    pub pre_total: i64,
}

/// A usage-limit notice.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LimitNotice {
    /// When, in UTC milliseconds.
    pub ts_ms: i64,
    /// The seat (or unseated session) label.
    pub seat: String,
    /// The harness's notice kind.
    pub subkind: Option<String>,
    /// The reset the notice named.
    pub resets_at: Option<String>,
}

/// How cost per call scales with context.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ContextCost {
    /// Warm calls.
    pub warm_calls: u64,
    /// Calls that rebuilt their cache.
    pub cold_calls: u64,
    /// Context growth per call (positive steps under 200k).
    pub growth_mean: f64,
    /// Median.
    pub growth_median: f64,
    /// List-price cost per warm call, binned by context (25k bins with at least 20 calls).
    pub bins: Vec<CostBin>,
    /// Linear fit of warm cost on context.
    pub fit: Fit,
    /// Growth runs: cumulative cost against the context the run has grown to.
    pub runs: Vec<RunCurve>,
    /// The square-law model: `[peak context, cumulative list USD]` of an average run.
    pub model: Vec<[f64; 2]>,
}

/// Warm calls of one 25k context bin.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CostBin {
    /// Bin centre.
    pub context: f64,
    /// Calls.
    pub calls: u64,
    /// Median list USD.
    pub median: f64,
    /// 25th percentile.
    pub p25: f64,
    /// 75th percentile.
    pub p75: f64,
    /// Median with cached reads free.
    pub reads_free_median: f64,
}

/// Least-squares fit of warm list-price cost per call on context.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Fit {
    /// USD per context token.
    pub slope: f64,
    /// USD at zero context.
    pub intercept: f64,
    /// Coefficient of determination.
    pub r2: f64,
}

/// One growth run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RunCurve {
    /// The seat (or unseated session) label.
    pub seat: String,
    /// The seat's role, when pij knows it.
    pub role: Option<String>,
    /// The harness that wrote the transcript (`claude-code`, `oh-my-pi`, …).
    pub harness: String,
    /// The harness's own session id, to find the transcript again.
    pub session_id: Option<String>,
    /// The model most of the run's calls used.
    pub model: Option<String>,
    /// First call, UTC ms.
    pub start_ms: i64,
    /// Last call, UTC ms.
    pub end_ms: i64,
    /// Calls in the run.
    pub calls: u64,
    /// Turns in the run.
    pub turns: u64,
    /// Of those, opened by an agent message.
    pub message_turns: u64,
    /// Of those, status turns (three calls or fewer).
    pub status_turns: u64,
    /// Idle cold wakes in the run.
    pub cold_wakes: u64,
    /// Of those, opened by a status turn.
    pub avoidable_cold_wakes: u64,
    /// List USD of the run.
    pub usd: f64,
    /// USD with cached reads free.
    pub usd_reads_free: f64,
    /// Largest context.
    pub peak_context: u64,
    /// `[context, cumulative list USD, cumulative reads-free USD]` at each new peak.
    pub points: Vec<[f64; 3]>,
    /// Status turns and cold wakes, where they happened on the curve.
    pub markers: Vec<RunMarker>,
    /// `[context, cumulative list USD]` of the same run replayed without its status turns.
    pub replay: Vec<[f64; 2]>,
}

/// A status turn or cold wake on a growth curve.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RunMarker {
    /// The context the run had reached.
    pub context: f64,
    /// Cumulative list USD at the marker.
    pub usd: f64,
    /// Cumulative reads-free USD at the marker.
    pub usd_reads_free: f64,
    /// `status`, `cold` (opened by real work) or `cold_avoidable` (opened by a status turn).
    pub kind: String,
    /// When, UTC ms.
    pub ts_ms: i64,
}

/// Growth runs on main seats, split by cost part.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SquareRule {
    /// Growth runs measured (or that reached the milestone).
    pub runs: u64,
    /// Median context growth per call.
    pub median_growth: f64,
    /// Median output per call.
    pub median_output: f64,
    /// Context where a call's reads cost more than its writes and output.
    pub crossover_context: f64,
    /// Cumulative cost at 200k/400k/600k/800k.
    pub milestones: Vec<Milestone>,
    /// Log-log slope from 200k to 600k: 2 is the square rule, 1 is linear.
    pub exponents: BTreeMap<String, f64>,
}

/// Median cumulative cost of the runs that reached `context`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Milestone {
    /// Context milestone, or consulted context.
    pub context: u64,
    /// Growth runs measured (or that reached the milestone).
    pub runs: u64,
    /// Median total USD.
    pub total: f64,
    /// Median cached-read USD.
    pub reads: f64,
    /// Median incremental-write USD.
    pub writes: f64,
    /// Median cold-rewrite USD.
    pub cold: f64,
    /// Median output USD.
    pub output: f64,
    /// Median uncached-input USD.
    pub input: f64,
    /// Median share of reads in the total (or reads, % of list).
    pub reads_share: f64,
}

/// Message-opened turns of three calls or fewer.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StatusTurns {
    /// Turns.
    pub turns: u64,
    /// Tokens by class.
    pub tokens: Tokens,
    /// The cache writes of the idle cold wakes status turns opened.
    pub cold_tokens: Tokens,
    /// Share of list spend, %.
    pub share: f64,
    /// Share without the cold wakes they opened, %.
    pub share_no_cold: f64,
    /// Share with cached reads free, %.
    pub share_reads_free: f64,
    /// Share with reads free, without their cold wakes, %.
    pub share_reads_free_no_cold: f64,
    /// Idle cold wakes opened by a status turn.
    pub avoidable_cold_wakes: u64,
    /// All idle cold wakes.
    pub idle_cold_wakes: u64,
    /// Idle cold wakes opened by real work.
    pub work_cold_wakes: u64,
    /// Their cache writes.
    pub work_cold_tokens: Tokens,
    /// List-price spend replayed without status turns.
    pub replay_total: f64,
    /// Share of list spend the replay saves, %.
    pub replay_saving_share: f64,
    /// Calls that go cold once their status turns are gone.
    pub replay_new_cold: u64,
}

/// Cold calls in the window.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ColdWakes {
    /// Cold calls after the cache expired.
    pub idle: u64,
    /// Their cache writes.
    pub idle_tokens: Tokens,
    /// Cold calls inside the cache lifetime (after a compaction or restart).
    pub rebuild: u64,
    /// Their cache writes.
    pub rebuild_tokens: Tokens,
    /// Idle cold calls opened by a peer message.
    pub idle_peer: u64,
    /// Their cache writes.
    pub idle_peer_tokens: Tokens,
    /// The costliest cold calls.
    pub top: Vec<ColdWake>,
}

/// What one consult of a cold seat costs, by model.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Consult {
    /// Main-session wake turns of 3–7 calls the per-call sizes were measured on.
    pub wake_turns: u64,
    /// Mean output per call.
    pub output_per_call: f64,
    /// Mean new (written) tokens per later call.
    pub new_per_call: f64,
    /// Share of idle cold wake turns whose seat made another call within the hour.
    pub follow_rate: f64,
    /// One row per context and call count.
    pub rows: Vec<ConsultRow>,
}

/// One consult size.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ConsultRow {
    /// Context milestone, or consulted context.
    pub context: u64,
    /// Calls.
    pub calls: u64,
    /// USD per model; `None` when the context does not fit the model's window.
    pub by_model: BTreeMap<String, Option<f64>>,
    /// Rewriting the context on the default model after a switch.
    pub switch_back: f64,
}

/// Headline figures at the default list price.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct KeyFigures {
    /// List USD of every call.
    pub total_usd: f64,
    /// USD with cached reads free.
    pub total_usd_reads_free: f64,
    /// Median share of reads in the total (or reads, % of list).
    pub reads_share: f64,
    /// Cold rewrites, % of list.
    pub cold_share: f64,
    /// Cold rewrites, % with reads free.
    pub cold_share_reads_free: f64,
    /// Incremental writes, % of list.
    pub incremental_share: f64,
    /// Output and input, % of list.
    pub output_share: f64,
    /// Peer-opened turns, % of list.
    pub peer_share: f64,
    /// Idle cold rewrites, list USD.
    pub cold_idle_usd: f64,
    /// Idle cold rewrites, % of list.
    pub cold_idle_share: f64,
    /// Idle cold rewrites, % with reads free.
    pub cold_idle_share_reads_free: f64,
    /// Simulated compactions, % of list plus compactions.
    pub compaction_share: f64,
    /// Status turns.
    pub status_turns: u64,
    /// Status turns, % of list.
    pub status_share: f64,
    /// Status turns without their cold wakes, % of list.
    pub status_share_no_cold: f64,
    /// Idle cold wakes opened by a status turn.
    pub avoidable_cold_wakes: u64,
    /// All idle cold wakes.
    pub idle_cold_wakes: u64,
    /// Share of list spend the replay saves, %.
    pub replay_saving_share: f64,
    /// Mean context per call.
    pub mean_context: u64,
    /// Turns opened by a peer message.
    pub peer_turns: u64,
    /// Turns opened by the human.
    pub human_turns: u64,
}
