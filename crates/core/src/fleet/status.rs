//! What no-action messages cost (status turns), and what consulting a cold seat costs.

use std::collections::BTreeMap;

use super::analysis::{Indexed, TurnAgg, pct, write_tokens};
use super::growth::by_source;
use super::{
    Call, Consult, ConsultRow, PriceTable, Prices, StatusTurns, TTL_MAIN_MS, TTL_SUB_MS, Tokens,
};

/// A replayed call that now costs this many times its recorded cost, and over $0.50, went cold.
const NEW_COLD_FACTOR: f64 = 5.0;
const NEW_COLD_MIN_USD: f64 = 0.5;
/// Consult sizes, after the RCA's `consult_costs.py`.
const CONSULT_CONTEXTS: [u64; 4] = [150_000, 300_000, 600_000, 900_000];
const CONSULT_CALLS: [u64; 3] = [3, 5, 7];
/// Wake turns of this many calls measure the consult's per-call sizes.
const WAKE_TURN_CALLS: std::ops::RangeInclusive<usize> = 3..=7;

fn turn_of<'a>(ix: &'a Indexed<'_>, call: &Call) -> Option<&'a TurnAgg> {
    ix.turns.get(&(call.source.clone(), call.turn_no))
}

pub(super) fn status_call(ix: &Indexed<'_>, call: &Call) -> bool {
    turn_of(ix, call).is_some_and(TurnAgg::status)
}

pub(super) fn status_turns(ix: &Indexed<'_>) -> StatusTurns {
    let p = &ix.prices;
    let mut out = StatusTurns::default();
    let mut total = Tokens::default();
    for call in &ix.calls {
        total.add(&call.tokens);
        let status = status_call(ix, call);
        if status {
            out.tokens.add(&call.tokens);
        }
        if call.cold() && call.idle() {
            out.idle_cold_wakes += 1;
            if status {
                out.cold_tokens.add(&write_tokens(&call.tokens));
            } else {
                out.work_cold_wakes += 1;
                out.work_cold_tokens.add(&write_tokens(&call.tokens));
            }
        }
    }
    let status_turns: Vec<&TurnAgg> = ix.turns.values().filter(|t| t.status()).collect();
    out.turns = status_turns.len() as u64;
    // A status turn counts once however many of its calls woke cold.
    out.avoidable_cold_wakes = status_turns.iter().filter(|t| t.idle_cold).count() as u64;
    let usd = total.usd(p);
    let usd_rf = total.usd_reads_free(p);
    let st = out.tokens.usd(p);
    let st_rf = out.tokens.usd_reads_free(p);
    let st_cold = out.cold_tokens.usd(p);
    out.share = pct(st, usd);
    out.share_no_cold = pct(st - st_cold, usd);
    out.share_reads_free = pct(st_rf, usd_rf);
    out.share_reads_free_no_cold = pct(st_rf - st_cold, usd_rf);

    let mut replay_total = 0.0;
    for calls in by_source(ix, false).values() {
        for (call, _, usd_after) in replay(ix, calls, p) {
            replay_total += usd_after;
            let recorded = call.tokens.usd(p);
            if !call.cold()
                && usd_after > recorded * NEW_COLD_FACTOR
                && usd_after > NEW_COLD_MIN_USD
            {
                out.replay_new_cold += 1;
            }
        }
    }
    out.replay_total = replay_total;
    out.replay_saving_share = pct(usd - replay_total, usd);
    out
}

/// One transcript replayed without its status turns: each kept call with its
/// context after the removal and its re-priced cost. A dropped call removes the context it added (its step over
/// the previous call's output, plus its own output) from every later call until
/// the next compaction; a kept call whose idle gap now outlives the cache goes cold.
pub(super) fn replay<'a>(
    ix: &Indexed<'_>,
    calls: &[&'a Call],
    p: &Prices,
) -> Vec<(&'a Call, u64, f64)> {
    let mut out = Vec::new();
    let mut removed: u64 = 0;
    let mut prev_kept_ts: Option<i64> = None;
    let mut prev_ctx: Option<u64> = None;
    let mut prev_out: u64 = 0;
    for call in calls {
        let ctx = call.context();
        if prev_ctx.is_some_and(|prev| ctx * 2 < prev) {
            removed = 0;
        }
        let out_usd =
            (call.tokens.output as f64 * p.output + call.tokens.input as f64 * p.input) / 1e6;
        let write_usd = write_tokens(&call.tokens).usd(p);
        if status_call(ix, call) {
            if let Some(prev) = prev_ctx {
                removed += ctx.saturating_sub(prev + prev_out) + call.tokens.output;
            }
            prev_ctx = Some(ctx);
            prev_out = call.tokens.output;
            continue;
        }
        let ttl = if call.is_sub { TTL_SUB_MS } else { TTL_MAIN_MS };
        let gap = match prev_kept_ts {
            Some(ts) => call.ts_ms - ts,
            None => call.gap_ms.unwrap_or(-1),
        };
        let kept_ctx = ctx.saturating_sub(removed);
        let usd = if call.cold() {
            let scale = if ctx == 0 {
                1.0
            } else {
                kept_ctx as f64 / ctx as f64
            };
            call.tokens.usd(p) * scale
        } else if gap > ttl && prev_kept_ts.is_some() {
            let rate = if call.is_sub { p.write_5m } else { p.write_1h };
            kept_ctx as f64 * rate / 1e6 + out_usd
        } else {
            call.tokens.cache_read.saturating_sub(removed) as f64 * p.read / 1e6
                + write_usd
                + out_usd
        };
        out.push((*call, kept_ctx, usd));
        prev_kept_ts = Some(call.ts_ms);
        prev_ctx = Some(ctx);
        prev_out = call.tokens.output;
    }
    out
}

/// One consult of a cold seat: call 1 writes the whole context and outputs `o`;
/// calls 2..k read the context so far, write `g` new tokens and output `o`.
fn consult_usd(p: &Prices, context: u64, calls: u64, o: f64, g: f64) -> Option<f64> {
    let ctx = context as f64;
    if ctx + calls as f64 * (o + g) > p.window as f64 {
        return None;
    }
    let mut usd = (ctx * p.write_1h + o * p.output) / 1e6;
    for i in 1..calls {
        usd += ((ctx + i as f64 * g) * p.read + g * p.write_1h + o * p.output) / 1e6;
    }
    Some(usd)
}

pub(super) fn consult(ix: &Indexed<'_>, prices: &PriceTable) -> Consult {
    let base = prices.base();
    let mut out = Consult::default();
    let (mut output, mut output_n, mut new, mut new_n) = (0.0, 0u64, 0.0, 0u64);
    let mut wakes = 0u64;
    let mut followed = 0u64;
    let sources = by_source(ix, true);
    for (key, turn) in ix.turns.iter().filter(|(_, t)| !t.is_sub && t.opened_cold) {
        wakes += 1;
        let calls: Vec<&&Call> = sources
            .get(turn.source.as_str())
            .map(|calls| calls.iter().filter(|c| c.turn_no == key.1).collect())
            .unwrap_or_default();
        let next = sources.get(turn.source.as_str()).and_then(|calls| {
            calls
                .iter()
                .find(|c| c.ts_ms > turn.last_ts && c.turn_no != key.1)
        });
        if next.is_some_and(|c| c.ts_ms - turn.last_ts <= TTL_MAIN_MS) {
            followed += 1;
        }
        if !WAKE_TURN_CALLS.contains(&turn.calls) {
            continue;
        }
        out.wake_turns += 1;
        for (i, call) in calls.iter().enumerate() {
            output += call.tokens.output as f64;
            output_n += 1;
            if i > 0 {
                new += call.tokens.writes() as f64;
                new_n += 1;
            }
        }
    }
    out.output_per_call = if output_n == 0 {
        0.0
    } else {
        output / output_n as f64
    };
    out.new_per_call = if new_n == 0 { 0.0 } else { new / new_n as f64 };
    out.follow_rate = if wakes == 0 {
        0.0
    } else {
        followed as f64 / wakes as f64
    };
    for context in CONSULT_CONTEXTS {
        for calls in CONSULT_CALLS {
            let by_model: BTreeMap<String, Option<f64>> = prices
                .models
                .iter()
                .map(|(name, p)| {
                    (
                        name.clone(),
                        consult_usd(p, context, calls, out.output_per_call, out.new_per_call),
                    )
                })
                .collect();
            out.rows.push(ConsultRow {
                context,
                calls,
                by_model,
                switch_back: context as f64 * (base.write_1h - base.read) / 1e6,
            });
        }
    }
    out
}
