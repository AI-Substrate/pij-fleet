//! How cost scales with context: per call (linear) and per growth run (the square rule).

use std::collections::BTreeMap;

use super::analysis::{Indexed, median};
use super::{Call, ContextCost, CostBin, Fit, Milestone, RunCurve, SquareRule};

/// Context bins of the per-call cost chart.
const BIN: u64 = 25_000;
/// A bin needs this many warm calls to be drawn.
const BIN_MIN_CALLS: usize = 20;
/// A context step this big or bigger is a restart, not growth.
const STEP_MAX: u64 = 200_000;
/// A growth run starts under this context…
const RUN_START_MAX: u64 = 150_000;
/// …and the cost chart draws runs that peak above this.
const RUN_PEAK_MIN: u64 = 400_000;
/// Square-rule milestones.
const MILES: [u64; 4] = [200_000, 400_000, 600_000, 800_000];
/// How many growth-run curves the report carries (costliest first).
const MAX_CURVES: usize = 200;

/// In-window calls of each main transcript, in order.
pub(super) fn by_source<'a>(ix: &Indexed<'a>, main_only: bool) -> BTreeMap<&'a str, Vec<&'a Call>> {
    let mut map: BTreeMap<&str, Vec<&Call>> = BTreeMap::new();
    for call in &ix.calls {
        if main_only && call.is_sub {
            continue;
        }
        map.entry(call.source.as_str()).or_default().push(call);
    }
    map
}

/// Split one transcript into growth runs: a call under half the previous
/// context is a compaction (or a clear) and starts a new run.
pub(super) fn runs<'a>(calls: &[&'a Call]) -> Vec<Vec<&'a Call>> {
    let mut runs = Vec::new();
    let mut current: Vec<&Call> = Vec::new();
    for call in calls {
        if let Some(last) = current.last()
            && call.context() * 2 < last.context()
        {
            runs.push(std::mem::take(&mut current));
        }
        current.push(call);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// `sorted[min(n - 1, p * n)]`, the RCA's quantile.
fn quantile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((p * sorted.len() as f64) as usize).min(sorted.len() - 1)]
}

pub(super) fn context_cost(ix: &Indexed<'_>) -> ContextCost {
    let p = &ix.prices;
    let mut out = ContextCost::default();
    let mut steps = Vec::new();
    for calls in by_source(ix, false).values() {
        for pair in calls.windows(2) {
            let (a, b) = (pair[0].context(), pair[1].context());
            if b > a && b - a < STEP_MAX {
                steps.push((b - a) as f64);
            }
        }
    }
    out.growth_mean = if steps.is_empty() {
        0.0
    } else {
        steps.iter().sum::<f64>() / steps.len() as f64
    };
    out.growth_median = median(&mut steps);

    let warm: Vec<&Call> = ix.calls.iter().copied().filter(|c| !c.cold()).collect();
    out.warm_calls = warm.len() as u64;
    out.cold_calls = ix.calls.len() as u64 - out.warm_calls;
    let mut bins: BTreeMap<u64, Vec<&Call>> = BTreeMap::new();
    for call in &warm {
        bins.entry(call.context() / BIN).or_default().push(call);
    }
    for (bin, calls) in bins {
        if calls.len() < BIN_MIN_CALLS {
            continue;
        }
        let mut usd: Vec<f64> = calls.iter().map(|c| c.tokens.usd(p)).collect();
        let mut rf: Vec<f64> = calls.iter().map(|c| c.tokens.usd_reads_free(p)).collect();
        let med = median(&mut usd);
        out.bins.push(CostBin {
            context: (bin as f64 + 0.5) * BIN as f64,
            calls: calls.len() as u64,
            median: med,
            p25: quantile(&usd, 0.25),
            p75: quantile(&usd, 0.75),
            reads_free_median: median(&mut rf),
        });
    }
    out.fit = fit(&warm, p);

    let mut curves = Vec::new();
    for (source, calls) in by_source(ix, true) {
        for run in runs(&calls) {
            let peak = run.iter().map(|c| c.context()).max().unwrap_or(0);
            if run[0].context() >= RUN_START_MAX || peak <= RUN_PEAK_MIN {
                continue;
            }
            let (mut acc, mut acc_rf, mut top) = (0.0, 0.0, 0);
            let mut points = Vec::new();
            for call in &run {
                acc += call.tokens.usd(p);
                acc_rf += call.tokens.usd_reads_free(p);
                if call.context() >= top {
                    top = call.context();
                    points.push([top as f64, acc, acc_rf]);
                }
            }
            curves.push((
                acc,
                RunCurve {
                    seat: ix.label(source),
                    points,
                },
            ));
        }
    }
    curves.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.runs = curves
        .into_iter()
        .take(MAX_CURVES)
        .map(|(_, curve)| curve)
        .collect();
    out
}

fn fit(warm: &[&Call], p: &super::Prices) -> Fit {
    let n = warm.len() as f64;
    if warm.len() < 2 {
        return Fit::default();
    }
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for call in warm {
        let (x, y) = (call.context() as f64, call.tokens.usd(p));
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let denom = n * sxx - sx * sx;
    if denom == 0.0 {
        return Fit::default();
    }
    let slope = (n * sxy - sx * sy) / denom;
    let intercept = (sy - slope * sx) / n;
    let mean = sy / n;
    let (mut tot, mut res) = (0.0, 0.0);
    for call in warm {
        let (x, y) = (call.context() as f64, call.tokens.usd(p));
        tot += (y - mean).powi(2);
        res += (y - (intercept + slope * x)).powi(2);
    }
    Fit {
        slope,
        intercept,
        r2: if tot == 0.0 { 0.0 } else { 1.0 - res / tot },
    }
}

/// Cumulative cost parts of a run at one milestone.
#[derive(Clone, Copy, Default)]
struct Parts {
    reads: f64,
    writes: f64,
    cold: f64,
    input: f64,
    output: f64,
}

impl Parts {
    fn total(&self) -> f64 {
        self.reads + self.writes + self.cold + self.input + self.output
    }
}

pub(super) fn square_rule(ix: &Indexed<'_>) -> SquareRule {
    let p = &ix.prices;
    let mut out = SquareRule::default();
    let mut at: BTreeMap<u64, Vec<Parts>> = BTreeMap::new();
    let mut growth = Vec::new();
    let mut outputs = Vec::new();
    for calls in by_source(ix, true).values() {
        for run in runs(calls) {
            let peak = run.iter().map(|c| c.context()).max().unwrap_or(0);
            if run[0].context() >= RUN_START_MAX || peak < MILES[1] {
                continue;
            }
            out.runs += 1;
            let mut acc = Parts::default();
            let mut next = 0;
            for call in &run {
                let t = &call.tokens;
                let writes = (t.cw_1h as f64 * p.write_1h + t.cw_5m as f64 * p.write_5m) / 1e6;
                acc.reads += t.cache_read as f64 * p.read / 1e6;
                if call.cold() {
                    acc.cold += writes;
                } else {
                    acc.writes += writes;
                }
                acc.input += t.input as f64 * p.input / 1e6;
                acc.output += t.output as f64 * p.output / 1e6;
                outputs.push(t.output as f64);
                while next < MILES.len() && call.context() >= MILES[next] {
                    at.entry(MILES[next]).or_default().push(acc);
                    next += 1;
                }
            }
            growth.push((peak - run[0].context()) as f64 / run.len() as f64);
        }
    }
    out.median_growth = median(&mut growth);
    out.median_output = median(&mut outputs);
    out.crossover_context = if p.read == 0.0 {
        0.0
    } else {
        (p.write_1h * out.median_growth + p.output * out.median_output) / p.read
    };
    let mut med: BTreeMap<u64, BTreeMap<&str, f64>> = BTreeMap::new();
    for mile in MILES {
        let Some(rows) = at.get(&mile).filter(|rows| rows.len() >= 5) else {
            continue;
        };
        let m = |f: &dyn Fn(&Parts) -> f64| median(&mut rows.iter().map(f).collect::<Vec<_>>());
        let milestone = Milestone {
            context: mile,
            runs: rows.len() as u64,
            total: m(&|x| x.total()),
            reads: m(&|x| x.reads),
            writes: m(&|x| x.writes),
            cold: m(&|x| x.cold),
            output: m(&|x| x.output),
            input: m(&|x| x.input),
            reads_share: m(&|x| {
                if x.total() == 0.0 {
                    0.0
                } else {
                    x.reads / x.total()
                }
            }),
        };
        med.insert(
            mile,
            BTreeMap::from([
                ("reads", milestone.reads),
                ("writes", milestone.writes),
                ("output", milestone.output),
                ("total", milestone.total),
                ("reads_free", m(&|x| x.total() - x.reads)),
            ]),
        );
        out.milestones.push(milestone);
    }
    if let (Some(a), Some(b)) = (med.get(&MILES[0]), med.get(&MILES[2])) {
        let span = (MILES[2] as f64 / MILES[0] as f64).ln();
        for (part, va) in a {
            let vb = b[part];
            if *va > 0.0 && vb > 0.0 {
                out.exponents
                    .insert((*part).to_string(), (vb / va).ln() / span);
            }
        }
    }
    out
}
