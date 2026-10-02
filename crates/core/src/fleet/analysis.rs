//! Every aggregate of the report, from the corpus.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::civil::{bucket_key, bucket_start, day_key};
use super::growth::{context_cost, square_rule};
use super::status::{consult, status_turns};
use super::{
    Bucket, Call, ColdWake, ColdWakes, CompactionSummary, Components, Corpus, Event, Graph,
    GraphEdge, GraphNode, KeyFigures, Lane, LaneCompaction, LanePoint, LimitNotice, PriceTable,
    Prices, REPORT_VERSION, Report, SenderRow, SeriesPoint, TTL_MAIN_MS, Tokens, Totals,
    TriggerRow, Window,
};

/// The RCA's trigger groups, in display order.
pub(super) const GROUPS: [&str; 7] = [
    "peer message",
    "Jordan (typed)",
    "background task done",
    "subagent",
    "limit reset resume",
    "loop / scheduled",
    "other",
];

/// The trigger group of a turn origin.
pub(super) fn group(origin: &str) -> &'static str {
    match origin {
        "peer" => "peer message",
        "human" => "Jordan (typed)",
        "task-notification" => "background task done",
        "subagent-task" | "coordinator" => "subagent",
        "auto-continuation" => "limit reset resume",
        "scheduled" | "loop" => "loop / scheduled",
        _ => "other",
    }
}

/// How many costliest main sessions get a context lane.
const LANES: usize = 14;
/// How many costliest cold calls the report lists.
const TOP_COLD: usize = 20;
/// How many senders the report lists.
const TOP_SENDERS: usize = 25;
/// The summary a compaction writes is priced as output, capped here.
const COMPACTION_SUMMARY_CAP: i64 = 30_000;
/// A recap (away summary) is priced as this much output.
const RECAP_OUTPUT: u64 = 300;

/// Who a transcript belongs to.
#[derive(Clone, Debug)]
pub(super) struct Label {
    pub name: String,
    pub seated: bool,
}

/// One turn's calls inside the window.
#[derive(Clone, Debug, Default)]
pub(super) struct TurnAgg {
    pub source: String,
    pub is_sub: bool,
    pub origin: String,
    pub sender: Option<String>,
    pub first_ts: i64,
    pub last_ts: i64,
    pub calls: usize,
    pub tokens: Tokens,
    /// Any call rebuilt its cache.
    pub cold: bool,
    pub cold_tokens: Tokens,
    /// Its first call was an idle cold wake.
    pub opened_cold: bool,
    /// Any call was an idle cold wake.
    pub idle_cold: bool,
}

impl TurnAgg {
    /// A message-opened turn that needed no action.
    pub fn status(&self) -> bool {
        self.origin == "peer" && self.calls <= super::STATUS_TURN_MAX_CALLS
    }
}

/// The corpus, indexed for the analyses.
pub(super) struct Indexed<'a> {
    pub window: Window,
    pub prices: Prices,
    /// In-window calls, ordered by (source, time, turn, call in turn).
    pub calls: Vec<&'a Call>,
    pub labels: HashMap<&'a str, Label>,
    pub turn_origin: HashMap<(&'a str, i64), (&'a str, Option<&'a str>)>,
    /// Turns keyed by (source, turn number), in call order.
    pub turns: BTreeMap<(String, i64), TurnAgg>,
    /// Seat roles by seat id.
    pub roles: HashMap<String, String>,
}

impl<'a> Indexed<'a> {
    pub fn label(&self, source: &str) -> String {
        self.labels
            .get(source)
            .map_or_else(|| format!("session {}", short(source)), |l| l.name.clone())
    }

    pub fn origin(&self, call: &Call) -> (&'a str, Option<&'a str>) {
        self.turn_origin
            .get(&(call.source.as_str(), call.turn_no))
            .copied()
            .unwrap_or(("start", None))
    }
}

/// The first eight characters, for unseated session labels.
fn short(id: &str) -> &str {
    let tail = id.rsplit('/').next().unwrap_or(id);
    let tail = tail.strip_suffix(".jsonl").unwrap_or(tail);
    match tail.char_indices().nth(8) {
        Some((end, _)) => &tail[..end],
        None => tail,
    }
}

/// Each transcript's label: its seat (by harness session id), or `session <id8>`.
fn labels(corpus: &Corpus) -> HashMap<&str, Label> {
    let mut seat_of: HashMap<&str, &str> = HashMap::new();
    for seat in &corpus.seats {
        for session in &seat.sessions {
            seat_of.entry(session.as_str()).or_insert(seat.id.as_str());
        }
    }
    corpus
        .sessions
        .iter()
        .map(|session| {
            let seat = session
                .session_id
                .as_deref()
                .and_then(|id| seat_of.get(id).copied());
            let label = match seat {
                Some(seat) => Label {
                    name: seat.to_string(),
                    seated: true,
                },
                None => Label {
                    name: format!(
                        "session {}",
                        short(session.session_id.as_deref().unwrap_or(&session.source))
                    ),
                    seated: false,
                },
            };
            (session.source.as_str(), label)
        })
        .collect()
}

/// Each transcript's label, as every table and chart of the report names it.
pub fn source_labels(corpus: &Corpus) -> HashMap<String, String> {
    labels(corpus)
        .into_iter()
        .map(|(source, label)| (source.to_string(), label.name))
        .collect()
}

fn index<'a>(corpus: &'a Corpus, window: Window, prices: &PriceTable) -> Indexed<'a> {
    let labels = labels(corpus);
    let turn_origin = corpus
        .turns
        .iter()
        .map(|turn| {
            (
                (turn.source.as_str(), turn.turn_no),
                (turn.origin.as_str(), turn.sender.as_deref()),
            )
        })
        .collect();
    let mut calls: Vec<&Call> = corpus
        .calls
        .iter()
        .filter(|call| window.contains(call.ts_ms))
        .collect();
    calls.sort_by(|a, b| {
        (&a.source, a.ts_ms, a.turn_no, a.call_in_turn).cmp(&(
            &b.source,
            b.ts_ms,
            b.turn_no,
            b.call_in_turn,
        ))
    });
    let mut indexed = Indexed {
        window,
        prices: prices.base(),
        calls,
        labels,
        turn_origin,
        turns: BTreeMap::new(),
        roles: corpus
            .seats
            .iter()
            .filter_map(|s| s.role.clone().map(|role| (s.id.clone(), role)))
            .collect(),
    };
    let mut turns: BTreeMap<(String, i64), TurnAgg> = BTreeMap::new();
    for call in &indexed.calls {
        let (origin, sender) = indexed.origin(call);
        let turn = turns
            .entry((call.source.clone(), call.turn_no))
            .or_insert_with(|| TurnAgg {
                source: call.source.clone(),
                is_sub: call.is_sub,
                origin: origin.to_string(),
                sender: sender.map(str::to_string),
                first_ts: call.ts_ms,
                opened_cold: call.cold() && call.idle(),
                ..TurnAgg::default()
            });
        turn.calls += 1;
        turn.last_ts = call.ts_ms;
        turn.tokens.add(&call.tokens);
        if call.cold() {
            turn.cold = true;
            turn.cold_tokens.add(&write_tokens(&call.tokens));
            turn.idle_cold |= call.idle();
        }
    }
    indexed.turns = turns;
    indexed
}

/// Only the cache writes of `tokens`.
pub(super) fn write_tokens(tokens: &Tokens) -> Tokens {
    Tokens {
        cw_1h: tokens.cw_1h,
        cw_5m: tokens.cw_5m,
        ..Tokens::default()
    }
}

/// The mutually exclusive parts of one call.
fn components(call: &Call) -> Components {
    let t = &call.tokens;
    let writes = write_tokens(t);
    let cold = call.cold();
    Components {
        warm_reads: Tokens {
            cache_read: t.cache_read,
            ..Tokens::default()
        },
        cold_rewrite: if cold { writes } else { Tokens::default() },
        incremental_writes: if cold { Tokens::default() } else { writes },
        output_cost: Tokens {
            input: t.input,
            output: t.output,
            ..Tokens::default()
        },
        ..Components::default()
    }
}

fn add_components(into: &mut Components, from: &Components) {
    into.warm_reads.add(&from.warm_reads);
    into.cold_rewrite.add(&from.cold_rewrite);
    into.incremental_writes.add(&from.incremental_writes);
    into.output_cost.add(&from.output_cost);
    into.compaction_sim.add(&from.compaction_sim);
    into.recap_sim.add(&from.recap_sim);
}

/// A compaction is not recorded as a call: it re-reads the history (written
/// again at the 5-minute rate when the cache had expired) and writes a summary.
fn compaction_tokens(event: &Event) -> (Tokens, bool) {
    let pre = event.pre_tokens.unwrap_or(0).max(0) as u64;
    let post = event
        .post_tokens
        .unwrap_or(0)
        .clamp(0, COMPACTION_SUMMARY_CAP) as u64;
    let cold = event
        .gap_ms
        .is_none_or(|gap| !(0..=TTL_MAIN_MS).contains(&gap));
    let tokens = Tokens {
        cw_5m: if cold { pre } else { 0 },
        cache_read: if cold { 0 } else { pre },
        output: post,
        ..Tokens::default()
    };
    (tokens, cold)
}

/// A recap (away summary) re-reads the context, warm or cold, and writes a short summary.
fn recap_tokens(event: &Event) -> Tokens {
    let context = event.last_context.unwrap_or(0).max(0) as u64;
    let warm = event.gap_ms.unwrap_or(0) <= TTL_MAIN_MS;
    Tokens {
        cache_read: if warm { context } else { 0 },
        cw_5m: if warm { 0 } else { context },
        output: RECAP_OUTPUT,
        ..Tokens::default()
    }
}

pub(super) fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

pub(super) fn pct(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        0.0
    } else {
        100.0 * part / whole
    }
}

/// Compute the report for `corpus` over `window`. Nonlinear statistics use the
/// price table's default model.
pub fn analyze(corpus: &Corpus, window: Window, prices: &PriceTable) -> Report {
    let ix = index(corpus, window, prices);
    let p = ix.prices.clone();
    let events: Vec<&Event> = corpus
        .events
        .iter()
        .filter(|event| event.ts_ms.is_some_and(|ts| window.contains(ts)))
        .collect();

    // ---- totals and the split ----
    let mut totals = Totals::default();
    let mut split = Components::default();
    let mut sources = BTreeSet::new();
    let mut context_sum = 0u64;
    for call in &ix.calls {
        totals.calls += 1;
        totals.tokens.add(&call.tokens);
        totals.cold_calls += u64::from(call.cold());
        context_sum += call.context();
        add_components(&mut split, &components(call));
        sources.insert(call.source.as_str());
    }
    totals.turns = ix.turns.len() as u64;
    totals.mean_context = context_sum.checked_div(totals.calls).unwrap_or(0);
    let main_sources: Vec<&str> = corpus
        .sessions
        .iter()
        .filter(|s| !s.is_sub && sources.contains(s.source.as_str()))
        .map(|s| s.source.as_str())
        .collect();
    totals.sessions = sources.len() as u64;
    totals.main_sessions = main_sources.len() as u64;
    totals.seated_sessions = main_sources
        .iter()
        .filter(|s| ix.labels.get(*s).is_some_and(|l| l.seated))
        .count() as u64;
    totals.unseated_sessions = totals.main_sessions - totals.seated_sessions;
    totals.seats = main_sources
        .iter()
        .filter_map(|s| ix.labels.get(*s).filter(|l| l.seated))
        .map(|l| l.name.as_str())
        .collect::<BTreeSet<_>>()
        .len() as u64;

    let mut compactions = CompactionSummary::default();
    let mut pres = Vec::new();
    for event in &events {
        match event.kind.as_str() {
            "compaction" => {
                let (tokens, cold) = compaction_tokens(event);
                split.compaction_sim.add(&tokens);
                compactions.count += 1;
                compactions.manual += u64::from(event.trigger.as_deref() == Some("manual"));
                compactions.cold += u64::from(cold);
                let pre = event.pre_tokens.unwrap_or(0);
                compactions.pre_total += pre;
                pres.push(pre as f64);
            }
            "recap" => split.recap_sim.add(&recap_tokens(event)),
            _ => {}
        }
    }
    compactions.median_pre = median(&mut pres) as i64;

    let (hourly, daily) = buckets(&ix, &events);
    let series_10m = series(&ix);
    let triggers = triggers(&ix);
    let senders = senders(&ix);
    let graph = graph(&ix, corpus);
    let lanes = lanes(&ix, corpus, &events);
    let limits = events
        .iter()
        .filter(|event| event.kind == "limit_notice")
        .map(|event| LimitNotice {
            ts_ms: event.ts_ms.unwrap_or_default(),
            seat: ix.label(&event.source),
            subkind: event.subkind.clone(),
            resets_at: event.resets_at.clone(),
        })
        .collect();
    let cold_wakes = cold_wakes(&ix);
    let status_turns = status_turns(&ix);
    let context_cost = context_cost(&ix);
    let square_rule = square_rule(&ix);
    let consult = consult(&ix, prices);

    let total = totals.tokens.usd(&p);
    let total_rf = totals.tokens.usd_reads_free(&p);
    let comp_usd = split.compaction_sim.usd(&p);
    let peer = triggers
        .iter()
        .find(|row| row.trigger == "peer message")
        .map_or(0.0, |row| row.tokens.usd(&p));
    let key_figures = KeyFigures {
        total_usd: total,
        total_usd_reads_free: total_rf,
        reads_share: pct(split.warm_reads.usd(&p), total),
        cold_share: pct(split.cold_rewrite.usd(&p), total),
        cold_share_reads_free: pct(split.cold_rewrite.usd(&p), total_rf),
        incremental_share: pct(split.incremental_writes.usd(&p), total),
        output_share: pct(split.output_cost.usd(&p), total),
        peer_share: pct(peer, total),
        cold_idle_usd: cold_wakes.idle_tokens.usd(&p),
        cold_idle_share: pct(cold_wakes.idle_tokens.usd(&p), total),
        cold_idle_share_reads_free: pct(cold_wakes.idle_tokens.usd(&p), total_rf),
        compaction_share: pct(comp_usd, total + comp_usd),
        status_turns: status_turns.turns,
        status_share: status_turns.share,
        status_share_no_cold: status_turns.share_no_cold,
        avoidable_cold_wakes: status_turns.avoidable_cold_wakes,
        idle_cold_wakes: status_turns.idle_cold_wakes,
        replay_saving_share: status_turns.replay_saving_share,
        mean_context: totals.mean_context,
        peer_turns: ix.turns.values().filter(|t| t.origin == "peer").count() as u64,
        human_turns: ix.turns.values().filter(|t| t.origin == "human").count() as u64,
    };

    Report {
        version: REPORT_VERSION,
        window: Some(window),
        scope: None,
        prices: prices.clone(),
        totals,
        components: split,
        hourly,
        daily,
        series_10m,
        triggers,
        senders,
        graph,
        lanes,
        compactions,
        limits,
        context_cost,
        square_rule,
        status_turns,
        cold_wakes,
        consult,
        key_figures,
    }
}

/// Hourly and daily buckets over the whole window, empty ones included.
fn buckets(ix: &Indexed<'_>, events: &[&Event]) -> (Vec<Bucket>, Vec<Bucket>) {
    let w = ix.window;
    let mut hourly: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut daily: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut t = bucket_start(w.since_ms, w.utc_offset_min, 60);
    while t < w.until_ms {
        for (map, key) in [
            (&mut hourly, bucket_key(t, w.utc_offset_min, 60)),
            (&mut daily, day_key(t, w.utc_offset_min)),
        ] {
            map.entry(key.clone()).or_insert_with(|| Bucket {
                key,
                ..Bucket::default()
            });
        }
        t += 3_600_000;
    }
    let mut hour_sessions: HashMap<String, BTreeSet<&str>> = HashMap::new();
    let mut day_sessions: HashMap<String, BTreeSet<&str>> = HashMap::new();
    for call in &ix.calls {
        let parts = components(call);
        let trigger = group(ix.origin(call).0);
        for (map, sessions, key) in [
            (
                &mut hourly,
                &mut hour_sessions,
                bucket_key(call.ts_ms, w.utc_offset_min, 60),
            ),
            (
                &mut daily,
                &mut day_sessions,
                day_key(call.ts_ms, w.utc_offset_min),
            ),
        ] {
            let Some(bucket) = map.get_mut(&key) else {
                continue;
            };
            bucket.calls += 1;
            bucket.cold += u64::from(call.cold());
            add_components(&mut bucket.components, &parts);
            bucket
                .by_trigger
                .entry(trigger.to_string())
                .or_default()
                .add(&call.tokens);
            sessions
                .entry(key)
                .or_default()
                .insert(call.source.as_str());
        }
    }
    for event in events {
        let ts = event.ts_ms.unwrap_or_default();
        for (map, key) in [
            (&mut hourly, bucket_key(ts, w.utc_offset_min, 60)),
            (&mut daily, day_key(ts, w.utc_offset_min)),
        ] {
            let Some(bucket) = map.get_mut(&key) else {
                continue;
            };
            match event.kind.as_str() {
                "compaction" => {
                    bucket.compactions += 1;
                    bucket
                        .components
                        .compaction_sim
                        .add(&compaction_tokens(event).0);
                }
                "recap" => bucket.components.recap_sim.add(&recap_tokens(event)),
                _ => {}
            }
        }
    }
    for turn in ix.turns.values() {
        for (map, key) in [
            (&mut hourly, bucket_key(turn.first_ts, w.utc_offset_min, 60)),
            (&mut daily, day_key(turn.first_ts, w.utc_offset_min)),
        ] {
            let Some(bucket) = map.get_mut(&key) else {
                continue;
            };
            bucket.peer_turns += u64::from(turn.origin == "peer");
            bucket.human_turns += u64::from(turn.origin == "human");
        }
    }
    for (map, sessions) in [(&mut hourly, &hour_sessions), (&mut daily, &day_sessions)] {
        for (key, set) in sessions {
            if let Some(bucket) = map.get_mut(key) {
                bucket.sessions = set.len() as u64;
            }
        }
    }
    (
        hourly.into_values().collect(),
        daily.into_values().collect(),
    )
}

fn series(ix: &Indexed<'_>) -> Vec<SeriesPoint> {
    let w = ix.window;
    let mut map: BTreeMap<String, Tokens> = BTreeMap::new();
    let mut t = bucket_start(w.since_ms, w.utc_offset_min, 10);
    while t < w.until_ms {
        map.insert(bucket_key(t, w.utc_offset_min, 10), Tokens::default());
        t += 600_000;
    }
    for call in &ix.calls {
        if let Some(tokens) = map.get_mut(&bucket_key(call.ts_ms, w.utc_offset_min, 10)) {
            tokens.add(&call.tokens);
        }
    }
    map.into_iter()
        .map(|(key, tokens)| SeriesPoint { key, tokens })
        .collect()
}

fn triggers(ix: &Indexed<'_>) -> Vec<TriggerRow> {
    let p = &ix.prices;
    let mut rows: Vec<TriggerRow> = GROUPS
        .iter()
        .map(|g| TriggerRow {
            trigger: (*g).to_string(),
            ..TriggerRow::default()
        })
        .collect();
    let mut per_turn: HashMap<&str, Vec<f64>> = HashMap::new();
    for call in &ix.calls {
        let g = group(ix.origin(call).0);
        if let Some(row) = rows.iter_mut().find(|row| row.trigger == g) {
            row.calls += 1;
            row.tokens.add(&call.tokens);
        }
    }
    for turn in ix.turns.values() {
        let g = group(&turn.origin);
        if let Some(row) = rows.iter_mut().find(|row| row.trigger == g) {
            row.turns += 1;
            row.cold_wakes += u64::from(turn.cold);
            row.cold_tokens.add(&turn.cold_tokens);
            per_turn.entry(g).or_default().push(turn.calls as f64);
        }
    }
    for row in &mut rows {
        if let Some(values) = per_turn.get_mut(row.trigger.as_str()) {
            row.median_calls_per_turn = median(values);
        }
    }
    rows.retain(|row| row.calls > 0);
    rows.sort_by(|a, b| b.tokens.usd(p).total_cmp(&a.tokens.usd(p)));
    rows
}

fn senders(ix: &Indexed<'_>) -> Vec<SenderRow> {
    let p = &ix.prices;
    let mut by: BTreeMap<String, (SenderRow, BTreeSet<&str>)> = BTreeMap::new();
    for turn in ix.turns.values().filter(|t| t.origin == "peer") {
        let name = turn.sender.clone().unwrap_or_default();
        let (row, recipients) = by.entry(name.clone()).or_insert_with(|| {
            (
                SenderRow {
                    sender: name,
                    ..SenderRow::default()
                },
                BTreeSet::new(),
            )
        });
        row.turns += 1;
        row.tokens.add(&turn.tokens);
        row.cold_wakes += u64::from(turn.cold);
        row.cold_tokens.add(&turn.cold_tokens);
        row.short_turns += u64::from(turn.status());
        recipients.insert(turn.source.as_str());
    }
    let mut rows: Vec<SenderRow> = by
        .into_values()
        .map(|(mut row, recipients)| {
            row.recipients = recipients.len() as u64;
            row
        })
        .collect();
    rows.sort_by(|a, b| {
        b.tokens
            .usd(p)
            .total_cmp(&a.tokens.usd(p))
            .then_with(|| a.sender.cmp(&b.sender))
    });
    rows.truncate(TOP_SENDERS);
    rows
}

fn graph(ix: &Indexed<'_>, corpus: &Corpus) -> Graph {
    let p = &ix.prices;
    let seats: HashMap<&str, &super::Seat> =
        corpus.seats.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
    for call in &ix.calls {
        let name = ix.label(&call.source);
        let node = nodes.entry(name.clone()).or_insert_with(|| GraphNode {
            id: name,
            ..GraphNode::default()
        });
        node.tokens.add(&call.tokens);
        node.max_context = node.max_context.max(call.context());
    }
    let mut edges: BTreeMap<(String, String), GraphEdge> = BTreeMap::new();
    for turn in ix
        .turns
        .values()
        .filter(|t| t.origin == "peer" && !t.is_sub)
    {
        let from = turn.sender.clone().unwrap_or_default();
        let to = ix.label(&turn.source);
        nodes.entry(from.clone()).or_insert_with(|| GraphNode {
            id: from.clone(),
            ..GraphNode::default()
        });
        let edge = edges
            .entry((from.clone(), to.clone()))
            .or_insert_with(|| GraphEdge {
                from,
                to,
                ..GraphEdge::default()
            });
        edge.turns += 1;
        edge.tokens.add(&turn.tokens);
        edge.cold += u64::from(turn.cold);
    }
    for node in nodes.values_mut() {
        if let Some(seat) = seats.get(node.id.as_str()) {
            node.seated = true;
            node.role = seat.role.clone();
            node.harness = Some(seat.harness.clone());
        }
    }
    let mut edges: Vec<GraphEdge> = edges.into_values().collect();
    edges.sort_by(|a, b| {
        b.tokens
            .usd(p)
            .total_cmp(&a.tokens.usd(p))
            .then_with(|| b.turns.cmp(&a.turns))
            .then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to)))
    });
    Graph {
        nodes: nodes.into_values().collect(),
        edges,
    }
}

pub(super) fn cold_wake(ix: &Indexed<'_>, call: &Call, status: bool) -> ColdWake {
    let (origin, sender) = ix.origin(call);
    ColdWake {
        ts_ms: call.ts_ms,
        seat: ix.label(&call.source),
        tokens: write_tokens(&call.tokens),
        gap_min: call.gap_ms.unwrap_or(-1) as f64 / 60_000.0,
        trigger: origin.to_string(),
        sender: sender.map(str::to_string),
        kind: if call.idle() { "idle" } else { "rebuild" }.to_string(),
        status,
    }
}

fn is_status_call(ix: &Indexed<'_>, call: &Call) -> bool {
    ix.turns
        .get(&(call.source.clone(), call.turn_no))
        .is_some_and(TurnAgg::status)
}

fn lanes(ix: &Indexed<'_>, corpus: &Corpus, events: &[&Event]) -> Vec<Lane> {
    let p = &ix.prices;
    let w = ix.window;
    let roles: HashMap<&str, Option<&str>> = corpus
        .seats
        .iter()
        .map(|s| (s.id.as_str(), s.role.as_deref()))
        .collect();
    let mut spend: BTreeMap<&str, f64> = BTreeMap::new();
    for call in ix.calls.iter().filter(|c| !c.is_sub) {
        *spend.entry(call.source.as_str()).or_default() += call.tokens.usd(p);
    }
    let mut ranked: Vec<(&str, f64)> = spend.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(LANES);
    ranked
        .into_iter()
        .map(|(source, _)| {
            let seat = ix.label(source);
            let mut lane = Lane {
                role: roles
                    .get(seat.as_str())
                    .copied()
                    .flatten()
                    .map(str::to_string),
                seat,
                ..Lane::default()
            };
            let mut points: BTreeMap<String, LanePoint> = BTreeMap::new();
            for call in ix.calls.iter().filter(|c| c.source == source) {
                lane.calls += 1;
                lane.tokens.add(&call.tokens);
                let key = bucket_key(call.ts_ms, w.utc_offset_min, 5);
                let point = points.entry(key.clone()).or_insert_with(|| LanePoint {
                    key,
                    ..LanePoint::default()
                });
                point.max_context = point.max_context.max(call.context());
                point.calls += 1;
                if call.cold() {
                    lane.cold
                        .push(cold_wake(ix, call, is_status_call(ix, call)));
                }
            }
            for turn in ix.turns.values().filter(|t| t.source == source) {
                lane.peer_turns += u64::from(turn.origin == "peer");
                lane.human_turns += u64::from(turn.origin == "human");
            }
            lane.compactions = events
                .iter()
                .filter(|e| e.kind == "compaction" && e.source == source)
                .map(|e| LaneCompaction {
                    ts_ms: e.ts_ms.unwrap_or_default(),
                    pre: e.pre_tokens.unwrap_or(0),
                    post: e.post_tokens.unwrap_or(0),
                })
                .collect();
            lane.points = points.into_values().collect();
            lane
        })
        .collect()
}

fn cold_wakes(ix: &Indexed<'_>) -> ColdWakes {
    let p = &ix.prices;
    let mut out = ColdWakes::default();
    let mut cold: Vec<&Call> = Vec::new();
    for call in ix.calls.iter().filter(|c| c.cold()) {
        let writes = write_tokens(&call.tokens);
        if call.idle() {
            out.idle += 1;
            out.idle_tokens.add(&writes);
            if ix.origin(call).0 == "peer" {
                out.idle_peer += 1;
                out.idle_peer_tokens.add(&writes);
            }
        } else {
            out.rebuild += 1;
            out.rebuild_tokens.add(&writes);
        }
        cold.push(call);
    }
    cold.sort_by(|a, b| {
        write_tokens(&b.tokens)
            .usd(p)
            .total_cmp(&write_tokens(&a.tokens).usd(p))
            .then_with(|| a.ts_ms.cmp(&b.ts_ms))
    });
    out.top = cold
        .into_iter()
        .take(TOP_COLD)
        .map(|call| cold_wake(ix, call, is_status_call(ix, call)))
        .collect();
    out
}
