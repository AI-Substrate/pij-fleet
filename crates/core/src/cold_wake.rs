//! The cold-wake guard (plan 157 phase 2, ruling #446; F214).
//!
//! Waking a seat whose prompt cache has expired re-writes its whole context at
//! cache-write prices. On a large seat that costs dollars per message, and the
//! 2026-09-28 usage blowout was made of exactly these wakes. So `pij send`
//! refuses a message to a seat that is cold (large, idle past its cache's real
//! lifetime, and not working), unless the sender forces it with a reason.
//!
//! **This is a brake, not a policy.** It can only refuse or warn. It never
//! changes what is delivered or how. Every fact it cannot establish (unsupported
//! harness, source failure, no transcript, unknown context or last-call time)
//! allows the send and says so. Removing the guard delivers the same messages
//! or more.
//!
//! **The TTL feeding it is a policy** (F214): the seat's own record first
//! (native, or derived from its native write split), then the pij cache proxy,
//! then a versioned table ([`crate::session_status::fill_cache_ttl`]). A wrong
//! TTL makes the brake refuse a warm seat or let a cold one through, so the
//! TTL's basis is shown wherever the verdict is.

use serde::{Deserialize, Serialize};

use crate::model::{SeatId, SystemState, is_copilot_fusion_model};
use crate::session_status::{Basis, CacheState, CacheTtl, Fact, SessionStatusBlock};

/// A seat holding more than this many context tokens is large enough to guard.
pub const COLD_CONTEXT_TOKENS: u64 = 400_000;

/// A seat whose last API call is older than this has lost its prompt cache,
/// whatever TTL it reports: the ceiling on every cache lifetime.
pub const COLD_IDLE_MS: u64 = 60 * 60 * 1_000;

/// What the guard does with a large seat idle past its cache TTL but not yet
/// past [`COLD_IDLE_MS`] (F214). One switch; every value is tested through
/// `decide`, so a ruling changes [`PAST_TTL`] and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PastTtl {
    /// Allow it with a warning; refuse only past [`COLD_IDLE_MS`] (option b).
    Warn,
    /// Refuse once idle reaches `ttl_percent`% of the TTL, warn before that
    /// (from the TTL on). `100` refuses at the TTL itself (option a); `150`
    /// and `200` give a provider's cache that outlives its stated TTL a grace
    /// (option c). [`COLD_IDLE_MS`] stays the ceiling whatever the product.
    RefuseAt {
        /// The refusal point as a percentage of the TTL.
        ttl_percent: u32,
    },
}

/// The rule in force: option (b) until Jordan rules (prime, 2026-11-19). The
/// F214 audit replays every option in the PR (#256) body.
pub const PAST_TTL: PastTtl = PastTtl::Warn;

/// A large seat whose cache TTL is unknown, idle longer than this, is warned
/// about: the shortest TTL in [`crate::session_status::CACHE_TTL_TABLE`].
pub const UNKNOWN_TTL_WARN_MS: u64 = 5 * 60 * 1_000;

/// The refusal's stable code.
pub const COLD_WAKE_CODE: &str = "E-RS-COLD-WAKE";

/// The spine kind recording every forced cold wake: the audit trail.
pub const COLD_WAKE_FORCED_KIND: &str = "send.cold-wake-forced";

/// Why a router seat's wake has no price (plan 169 R7): the router picks the
/// model per call, so no single model's list price describes the wake.
pub const ROUTER_NOT_ESTIMABLE: &str = "not estimable: router, per-call models";

/// One model's list prices, in US dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    /// A 1-hour prompt-cache write (what Claude Code writes on Max).
    pub cache_write_1h: f64,
    /// A prompt-cache read.
    pub cache_read: f64,
    /// Output.
    pub output: f64,
}

/// List prices (platform.claude.com, checked 2026-10-01). ESTIMATES for the
/// refusal and the seat views only; they never decide anything. Keys are
/// [`price_key`] form: no provider prefix, `-` between version numbers.
pub const PRICES_USD_PER_MTOK: &[(&str, ModelPrice)] = &[
    (
        "claude-opus-5-5",
        ModelPrice {
            cache_write_1h: 8.0,
            cache_read: 0.20,
            output: 20.0,
        },
    ),
    (
        "claude-sonnet-5-5",
        ModelPrice {
            cache_write_1h: 4.0,
            cache_read: 0.20,
            output: 10.0,
        },
    ),
    (
        "claude-haiku-4-5",
        ModelPrice {
            cache_write_1h: 2.0,
            cache_read: 0.10,
            output: 5.0,
        },
    ),
];

/// How many calls a wake turn is priced at, and each call's mean new and
/// output tokens: the 2026-09-28 usage study's 3-7-call wake turns
/// (`~/games/unasphere/scratch/usage-blowout/consult_costs.py`).
pub const WAKE_CALLS: u64 = 5;
/// Mean tokens each call after the first writes to the cache.
pub const WAKE_NEW_TOKENS_PER_CALL: u64 = 1_585;
/// Mean output tokens per call.
pub const WAKE_OUTPUT_TOKENS_PER_CALL: u64 = 590;

/// What waking a cold seat costs at list price, when its model is priced: the
/// first call writes the whole context to the 1-hour cache, and each of the
/// other [`WAKE_CALLS`] - 1 calls reads the context so far and writes its new
/// tokens; every call outputs. An estimate, never a decision.
pub fn wake_estimate_usd(model: &str, context_tokens: u64) -> Option<f64> {
    let key = price_key(model);
    let (_, price) = PRICES_USD_PER_MTOK
        .iter()
        .find(|(priced, _)| *priced == key)?;
    let output = WAKE_OUTPUT_TOKENS_PER_CALL as f64 * price.output;
    let mut micro_usd = context_tokens as f64 * price.cache_write_1h + output;
    for call in 1..WAKE_CALLS {
        let context_so_far = context_tokens + call * WAKE_NEW_TOKENS_PER_CALL;
        micro_usd += context_so_far as f64 * price.cache_read
            + WAKE_NEW_TOKENS_PER_CALL as f64 * price.cache_write_1h
            + output;
    }
    Some(micro_usd / 1_000_000.0)
}

/// The price-table key for a harness's model id. Harnesses name one model
/// differently: Claude Code says `claude-opus-5-5`, OMP says
/// `github-copilot/claude-opus-5.5`. Drop any provider prefix and spell the
/// version with dashes.
pub fn price_key(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).replace('.', "-")
}

/// What the guard concluded for one recipient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum ColdCheck {
    /// Not cold: small, recently called, or both.
    Clear,
    /// The daemon sees the seat working, so it is warm by definition.
    Busy,
    /// Large and idle past its cache lifetime: refuse unless forced.
    #[serde(rename_all = "camelCase")]
    Cold {
        /// Tokens the wake would re-write.
        context_tokens: u64,
        /// Milliseconds since the seat's last API call.
        idle_ms: u64,
        /// The seat's current model, when known; a router seat's router id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Estimated list cost of the wake ([`wake_estimate_usd`]), when the
        /// model is priced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimate_usd: Option<f64>,
        /// Why no estimate can exist ([`ROUTER_NOT_ESTIMABLE`]), as opposed to
        /// a single model that is merely unpriced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        not_estimable: Option<String>,
        /// The cache TTL and its basis (`5m native`), when known (F214).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_ttl: Option<String>,
    },
    /// Large and idle long enough that the wake may be cold, but not refused
    /// (F214): the TTL is unknown, or [`PAST_TTL`] warns past the TTL. The
    /// send goes through and the receipt says why.
    #[serde(rename_all = "camelCase")]
    Warn {
        /// Tokens the wake may re-write.
        context_tokens: u64,
        /// Milliseconds since the seat's last API call.
        idle_ms: u64,
        /// Why it may be cold.
        why: String,
    },
    /// A fact the rule needs is unknown, so the send is allowed.
    Unknown {
        /// Which fact, or which failure.
        why: String,
    },
}

impl ColdCheck {
    /// The receipt's `cold_check` line: `clear`, `busy`, `forced`, `cold`,
    /// `warn: <why>` or `unknown: <why>`.
    pub fn receipt_label(&self, forced: bool) -> String {
        match self {
            Self::Clear => "clear".to_string(),
            Self::Busy => "busy".to_string(),
            Self::Cold { .. } if forced => "forced".to_string(),
            Self::Cold { .. } => "cold".to_string(),
            Self::Warn { why, .. } => format!("warn: {why}"),
            Self::Unknown { why } => format!("unknown: {why}"),
        }
    }
}

/// `5m native`, `30m native`, `1h proxy`, `5m table@v1`: a TTL with its basis.
pub fn ttl_text(ttl: &Fact<CacheTtl>) -> Option<String> {
    match ttl {
        Fact::Known { value, basis } => Some(format!("{} {}", value.label(), basis.to_wire())),
        Fact::Unknown => None,
    }
}

/// Decide one recipient under [`PAST_TTL`]. `state` is the daemon's own
/// busy/idle observation; `status` is the `pij state` session block; `now_ms`
/// is the daemon clock; `seat_model` is the model the seat was bound with.
pub fn check(
    state: SystemState,
    status: &SessionStatusBlock,
    now_ms: u64,
    seat_model: Option<&str>,
) -> ColdCheck {
    decide(PAST_TTL, state, status, now_ms, seat_model)
}

/// [`check`] under either [`PastTtl`] rule, so both stay tested whichever is
/// in force.
fn decide(
    rule: PastTtl,
    state: SystemState,
    status: &SessionStatusBlock,
    now_ms: u64,
    seat_model: Option<&str>,
) -> ColdCheck {
    if state == SystemState::Working {
        return ColdCheck::Busy;
    }
    let status = match status {
        SessionStatusBlock::Known { status, .. } => status,
        SessionStatusBlock::Unbound => return unknown("no harness session bound"),
        SessionStatusBlock::Unsupported { harness } => {
            return unknown(&format!("{} sessions are not readable", harness.as_str()));
        }
        SessionStatusBlock::NotFound { .. } => return unknown("no transcript found"),
        SessionStatusBlock::Failed { error } => return unknown(&format!("source failed: {error}")),
    };
    let Some(&context_tokens) = status.context_used_tokens.value() else {
        return unknown("context size unknown");
    };
    if context_tokens <= COLD_CONTEXT_TOKENS {
        return ColdCheck::Clear;
    }
    let Some(&last_call) = status.last_call_at_ms.value() else {
        return unknown("last call time unknown");
    };
    // A last call stamped in the future (clock skew) counts as just made.
    let idle_ms = now_ms.saturating_sub(last_call);
    let cache_ttl = ttl_text(&status.cache_ttl);
    if idle_ms <= COLD_IDLE_MS {
        let warn = |why: String| ColdCheck::Warn {
            context_tokens,
            idle_ms,
            why,
        };
        match (status.cache_ttl.value(), &cache_ttl) {
            // Inside its lifetime: warm. `cache_state` calls it cold at the TTL.
            (Some(ttl), _) if idle_ms < ttl.as_ms() => return ColdCheck::Clear,
            (Some(ttl), Some(text)) => {
                let refuse_at = match rule {
                    PastTtl::Warn => None,
                    PastTtl::RefuseAt { ttl_percent } => {
                        Some(ttl.as_ms().saturating_mul(u64::from(ttl_percent)) / 100)
                    }
                };
                if refuse_at.is_none_or(|refuse_at| idle_ms < refuse_at) {
                    return warn(format!(
                        "{} idle {}, past its {text} cache; the wake re-writes it",
                        human_tokens(context_tokens),
                        human_duration(idle_ms),
                    ));
                }
            }
            (Some(_), None) => {}
            (None, _) if idle_ms > UNKNOWN_TTL_WARN_MS => {
                return warn(format!(
                    "cache TTL unknown; {} idle {} may be cold",
                    human_tokens(context_tokens),
                    human_duration(idle_ms),
                ));
            }
            (None, _) => return ColdCheck::Clear,
        }
    }
    let model = match &status.model {
        Fact::Known { value, .. } => Some(value.clone()),
        Fact::Unknown => None,
    };
    // A router seat's last served model is one of several it may route to, so
    // pricing the wake at that model would be a guess presented as a number.
    let router = seat_model
        .filter(|model| is_copilot_fusion_model(model))
        .or(model
            .as_deref()
            .filter(|model| is_copilot_fusion_model(model)));
    if let Some(router) = router {
        return ColdCheck::Cold {
            context_tokens,
            idle_ms,
            model: Some(router.to_string()),
            estimate_usd: None,
            not_estimable: Some(ROUTER_NOT_ESTIMABLE.to_string()),
            cache_ttl,
        };
    }
    let estimate_usd = model
        .as_deref()
        .and_then(|model| wake_estimate_usd(model, context_tokens));
    ColdCheck::Cold {
        context_tokens,
        idle_ms,
        model,
        estimate_usd,
        not_estimable: None,
        cache_ttl,
    }
}

/// A wake's price as shown: the estimate, the reason none can exist, or unknown.
fn wake_price(estimate_usd: Option<f64>, not_estimable: Option<&str>) -> String {
    match (estimate_usd, not_estimable) {
        (Some(usd), _) => format!("~${usd:.2}"),
        (None, Some(why)) => format!("? ({why})"),
        (None, None) => "price unknown".to_string(),
    }
}

fn unknown(why: &str) -> ColdCheck {
    ColdCheck::Unknown {
        why: why.to_string(),
    }
}

/// Is the seat's prompt cache known WARM (plan 159's FYI flush)? A flush is a
/// wake nobody asked for, so this needs POSITIVE evidence: the last API call is
/// known and within the seat's real cache lifetime, `min(cache TTL, COLD_IDLE_MS)`.
/// A `working` state is not evidence (it can be stale after an Esc or an API
/// error), and an unknown last call, an unknown TTL or a TTL from the table
/// alone is not warm.
pub fn is_warm(status: &SessionStatusBlock, now_ms: u64) -> bool {
    cache_known(status, now_ms) == Ok(true)
}

/// The prompt cache's state at `now_ms` when the facts establish it: `Ok(true)`
/// warm, `Ok(false)` cold. Otherwise `Err` names, from a closed vocabulary, the
/// fact that is missing, for a caller that must act only on a known state
/// (plan 225's `revive --cheap`, plan 159's FYI flush). An action needs the
/// seat's own evidence, so a TTL only the table supplies (F214) is unknown
/// here; the guard, a brake, may still use it.
///
/// # Errors
/// The cache state is unknown: the named fact could not be established.
pub fn cache_known(status: &SessionStatusBlock, now_ms: u64) -> Result<bool, &'static str> {
    let status = match status {
        SessionStatusBlock::Known { status, .. } => status,
        SessionStatusBlock::Unbound => return Err("no session bound"),
        SessionStatusBlock::Unsupported { .. } => return Err("harness sessions unreadable"),
        SessionStatusBlock::NotFound { .. } => return Err("no transcript found"),
        SessionStatusBlock::Failed { .. } => return Err("session read failed"),
    };
    let Some(&last_call) = status.last_call_at_ms.value() else {
        return Err("last call unknown");
    };
    let ttl = match &status.cache_ttl {
        Fact::Known {
            basis: Basis::Table { .. },
            ..
        } => return Err("cache TTL from the table only"),
        Fact::Known { value, .. } => *value,
        Fact::Unknown => return Err("cache TTL unknown"),
    };
    let lifetime_ms = ttl.as_ms().min(COLD_IDLE_MS);
    Ok(now_ms < last_call.saturating_add(lifetime_ms))
}

/// The refusal a sender sees: the seat's size and idle time, then every option
/// the sender has, each priced. One row per option, so a new one is a new row.
pub fn refusal(recipient: &SeatId, check: &ColdCheck) -> Option<String> {
    let ColdCheck::Cold {
        context_tokens,
        idle_ms,
        estimate_usd,
        not_estimable,
        cache_ttl,
        ..
    } = check
    else {
        return None;
    };
    let wake = wake_price(*estimate_usd, not_estimable.as_deref());
    let options = [
        // F169: an FYI is for a recipient with nothing to do; it wakes no one,
        // so it is no way to hand work to a cold seat.
        (
            "--fyi",
            "only if they need do nothing; wakes no one",
            "$0 now".to_string(),
        ),
        ("--force --reason \"…\"", "wake on its current model", wake),
    ];
    let cache = cache_ttl
        .as_deref()
        .map(|ttl| format!(", cache {ttl}"))
        .unwrap_or_default();
    let mut text = format!(
        "{COLD_WAKE_CODE}: ❄ {recipient} is cold ({}, idle {}{cache}). Options:",
        human_tokens(*context_tokens),
        human_duration(*idle_ms),
    );
    for (flag, effect, price) in options {
        text.push_str(&format!("\n  {flag:<22} {effect:<43} {price}"));
    }
    text.push_str(&format!(
        "\nEstimates at list price: a 1-hour cache write of the whole context, plus about {WAKE_CALLS} calls."
    ));
    Some(text)
}

/// `14m`, `2h`, `2h14m`: a duration at minute resolution.
pub fn human_duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h{minutes:02}m"),
    }
}

/// `375k`, `1M`, `1.5M`: a token count at thousand resolution.
pub fn human_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        format!("{}k", tokens / 1_000)
    }
}

/// What `pij list` and `pij state` show about a seat's size and coldness
/// (plan 160), derived from its session facts on the daemon clock. A field is
/// absent when its fact is unknown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatSize {
    /// Context tokens in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    /// Milliseconds since the seat's last API call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
    /// Whether the prompt cache is warm or cold, and for how long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_state: Option<CacheState>,
    /// What the cold-wake guard would do with a normal send right now.
    #[serde(default)]
    pub cold_wake: ColdWakeView,
}

/// The cold-wake guard's verdict for a normal send right now ([`check`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColdWakeView {
    /// The guard would refuse a normal send.
    pub would_refuse: bool,
    /// The wake's list-price estimate when it would refuse and the model is priced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_usd: Option<f64>,
    /// Why no estimate can exist, e.g. [`ROUTER_NOT_ESTIMABLE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_estimable: Option<String>,
    /// Why a normal send would go through with a warning ([`ColdCheck::Warn`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Derive a seat's [`SeatSize`]. The refusal verdict is [`check`] itself, so
/// a ❄ row is exactly a seat the guard would refuse now.
pub fn seat_size(
    state: SystemState,
    status: &SessionStatusBlock,
    now_ms: u64,
    seat_model: Option<&str>,
) -> SeatSize {
    let cold_wake = match check(state, status, now_ms, seat_model) {
        ColdCheck::Cold {
            estimate_usd,
            not_estimable,
            ..
        } => ColdWakeView {
            would_refuse: true,
            estimate_usd,
            not_estimable,
            warning: None,
        },
        ColdCheck::Warn { why, .. } => ColdWakeView {
            warning: Some(why),
            ..ColdWakeView::default()
        },
        _ => ColdWakeView::default(),
    };
    let SessionStatusBlock::Known {
        status,
        cache_state,
        ..
    } = status
    else {
        return SeatSize {
            cold_wake,
            ..SeatSize::default()
        };
    };
    SeatSize {
        context_used: status.context_used_tokens.value().copied(),
        idle_ms: status
            .last_call_at_ms
            .value()
            .map(|&last_call| now_ms.saturating_sub(last_call)),
        cache_state: cache_state.value().cloned(),
        cold_wake,
    }
}

/// The list columns CTX, IDLE and CACHE, `?` where unknown, and the marker:
/// ❄ the guard would refuse, ⚠ it would warn.
pub fn size_columns(size: &SeatSize) -> [String; 4] {
    [
        size.context_used
            .map_or_else(|| "?".to_string(), human_tokens),
        size.idle_ms.map_or_else(|| "?".to_string(), human_duration),
        size.cache_state
            .as_ref()
            .map_or_else(|| "?".to_string(), cache_text),
        if size.cold_wake.would_refuse {
            "❄"
        } else if size.cold_wake.warning.is_some() {
            "⚠"
        } else {
            ""
        }
        .to_string(),
    ]
}

fn cache_text(state: &CacheState) -> String {
    match state {
        CacheState::Warm { .. } => "warm".to_string(),
        CacheState::Cold { expired_for_ms } => format!("cold {}", human_duration(*expired_for_ms)),
    }
}

/// `pij state`'s size line and, when the guard would refuse or warn, its ❄ or
/// ⚠ line: `context 375k / 1M · last call 14m ago · cache 5m native (cold 9m) ·
/// 3 compactions`. The TTL names its basis (F214). Nothing when the facts
/// could not be read (the `session:` line says why).
pub fn size_lines(status: &SessionStatusBlock, size: &SeatSize) -> Vec<String> {
    let SessionStatusBlock::Known { status, .. } = status else {
        return Vec::new();
    };
    let known = |value: Option<String>| value.unwrap_or_else(|| "?".to_string());
    let cache = match (ttl_text(&status.cache_ttl), &size.cache_state) {
        (Some(ttl), Some(state)) => format!("{ttl} ({})", cache_text(state)),
        (Some(ttl), None) => ttl,
        (None, _) => "?".to_string(),
    };
    let mut lines = vec![format!(
        "context {} / {} · last call {} · cache {cache} · {} compactions",
        known(size.context_used.map(human_tokens)),
        known(
            status
                .context_window_tokens
                .value()
                .copied()
                .map(human_tokens)
        ),
        known(size.idle_ms.map(|ms| format!("{} ago", human_duration(ms)))),
        known(status.compactions.value().map(u64::to_string)),
    )];
    if size.cold_wake.would_refuse {
        let price = wake_price(
            size.cold_wake.estimate_usd,
            size.cold_wake.not_estimable.as_deref(),
        );
        lines.push(format!(
            "❄ cold-wake guard: a normal send is refused; waking it costs {price} (--fyi only if it has nothing to do; wakes no one)"
        ));
    }
    if let Some(why) = &size.cold_wake.warning {
        lines.push(format!(
            "⚠ cold-wake guard: a normal send goes through with a warning: {why}"
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;
    use crate::session_status::{Fact, SeatStatus};

    const NOW: u64 = 10 * 60 * 60 * 1_000;

    fn known(
        context: Option<u64>,
        last_call: Option<u64>,
        model: Option<&str>,
    ) -> SessionStatusBlock {
        SessionStatusBlock::Known {
            status: Box::new(SeatStatus {
                model: model.map_or(Fact::Unknown, |m| Fact::native(m.to_string())),
                context_used_tokens: context.map_or(Fact::Unknown, Fact::derived),
                last_call_at_ms: last_call.map_or(Fact::Unknown, Fact::native),
                ..SeatStatus::unknown()
            }),
            cache_state: Fact::Unknown,
            elapsed_ms: 0,
        }
    }

    /// The same model reported three ways (Claude Code, OMP's provider-qualified
    /// id, a dotted version) is one price, not "unknown".
    #[test]
    fn a_provider_qualified_or_dotted_model_id_finds_its_price() {
        let idle = 2 * COLD_IDLE_MS;
        for model in [
            "claude-opus-5-5",
            "claude-opus-5.5",
            "github-copilot/claude-opus-5.5",
            "anthropic/claude-opus-5-5",
        ] {
            let ColdCheck::Cold { estimate_usd, .. } = check(
                SystemState::Idle,
                &known(Some(500_000), Some(NOW - idle), Some(model)),
                NOW,
                None,
            ) else {
                panic!("{model}: expected cold");
            };
            let usd = estimate_usd.expect(model);
            assert!((usd - 4.51289).abs() < 1e-9, "{model}: {usd}");
        }
    }

    /// A full card's facts: 720k of a 1M window, Opus, a 1-hour cache,
    /// three compactions, last call `ago` ms before NOW.
    fn full(ago: u64) -> SessionStatusBlock {
        let status = SeatStatus {
            model: Fact::native("claude-opus-5-5".to_string()),
            context_used_tokens: Fact::derived(720_000),
            context_window_tokens: Fact::derived(1_000_000),
            last_call_at_ms: Fact::native(NOW - ago),
            cache_ttl: Fact::derived(CacheTtl::ONE_HOUR),
            compactions: Fact::native(3),
            ..SeatStatus::unknown()
        };
        SessionStatusBlock::Known {
            cache_state: crate::session_status::cache_state(&status, NOW),
            status: Box::new(status),
            elapsed_ms: 0,
        }
    }

    /// Plan 160: ❄ is exactly the guard's refusal, never a restatement of it.
    #[test]
    fn a_seat_is_marked_cold_exactly_when_the_guard_would_refuse() {
        let two_hours = 2 * COLD_IDLE_MS;
        let cold = seat_size(SystemState::Idle, &full(two_hours), NOW, None);
        assert!(cold.cold_wake.would_refuse);
        assert_eq!(
            cold.cold_wake.estimate_usd,
            wake_estimate_usd("claude-opus-5-5", 720_000)
        );
        assert_eq!(
            size_columns(&cold),
            ["720k", "2h", "cold 1h", "❄"].map(String::from)
        );
        // Working, or called recently: the guard allows, so no ❄.
        for (state, ago) in [
            (SystemState::Working, two_hours),
            (SystemState::Idle, 60_000),
        ] {
            let size = seat_size(state, &full(ago), NOW, None);
            assert!(!size.cold_wake.would_refuse, "{state:?} {ago}");
            assert_eq!(size_columns(&size)[3], "");
        }
        let warm = seat_size(SystemState::Idle, &full(60_000), NOW, None);
        assert_eq!(
            size_columns(&warm),
            ["720k", "1m", "warm", ""].map(String::from)
        );
        // Nothing known: every column is `?`, never a guess.
        let unknown = seat_size(SystemState::Idle, &SessionStatusBlock::Unbound, NOW, None);
        assert_eq!(
            size_columns(&unknown),
            ["?", "?", "?", ""].map(String::from)
        );
    }

    #[test]
    fn the_state_lines_name_size_cache_and_the_guard() {
        let block = full(2 * COLD_IDLE_MS);
        let size = seat_size(SystemState::Idle, &block, NOW, None);
        assert_eq!(
            size_lines(&block, &size),
            [
                "context 720k / 1M · last call 2h ago · cache 1h derived (cold 1h) · 3 compactions",
                "❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi only if it has nothing to do; wakes no one)",
            ]
            .map(String::from)
        );
        let warm = full(60_000);
        assert_eq!(
            size_lines(&warm, &seat_size(SystemState::Idle, &warm, NOW, None)),
            [
                "context 720k / 1M · last call 1m ago · cache 1h derived (warm) · 3 compactions"
                    .to_string()
            ]
        );
        assert!(size_lines(&SessionStatusBlock::Unbound, &SeatSize::default()).is_empty());
        assert_eq!(human_tokens(1_000_000), "1M");
        assert_eq!(human_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn warm_needs_a_known_last_call_inside_the_real_cache_lifetime() {
        let at = |ago: u64, ttl: Fact<CacheTtl>| SessionStatusBlock::Known {
            status: Box::new(SeatStatus {
                last_call_at_ms: Fact::native(NOW - ago),
                cache_ttl: ttl,
                ..SeatStatus::unknown()
            }),
            cache_state: Fact::Unknown,
            elapsed_ms: 0,
        };
        let five = CacheTtl::FIVE_MINUTES;
        let hour = CacheTtl::ONE_HOUR;
        assert!(is_warm(&at(five.as_ms() - 1, Fact::derived(five)), NOW));
        assert!(
            !is_warm(&at(five.as_ms(), Fact::derived(five)), NOW),
            "expired at its lifetime"
        );
        assert!(is_warm(&at(COLD_IDLE_MS - 1, Fact::derived(hour)), NOW));
        assert!(!is_warm(&at(COLD_IDLE_MS, Fact::derived(hour)), NOW));
        // F214: a GPT seat's own 30-minute TTL is evidence; a 2-minute idle is warm.
        let thirty = CacheTtl::from_seconds(1_800).expect("30m");
        assert!(is_warm(&at(2 * 60_000, Fact::native(thirty)), NOW));
        assert_eq!(
            cache_known(&at(31 * 60_000, Fact::native(thirty)), NOW),
            Ok(false)
        );
        // The proxy rung is the seat's own route; the table is not its evidence,
        // so an action (flush, revive --cheap) never acts on it.
        let proxy = Fact::Known {
            value: hour,
            basis: Basis::Proxy,
        };
        assert!(is_warm(&at(60_000, proxy), NOW));
        let table = Fact::Known {
            value: hour,
            basis: Basis::Table { version: 1 },
        };
        assert_eq!(
            cache_known(&at(60_000, table), NOW),
            Err("cache TTL from the table only")
        );
        // Unknown TTL, unknown last call, or no facts: never warm.
        assert!(!is_warm(&known(None, Some(NOW - 1), None), NOW));
        assert!(!is_warm(&known(Some(10), None, None), NOW));
        assert!(!is_warm(&SessionStatusBlock::Unbound, NOW));
    }

    #[test]
    fn a_large_seat_idle_past_the_ttl_is_cold_with_its_price() {
        let idle = 112 * 60 * 1_000;
        let check = check(
            SystemState::Idle,
            &known(Some(720_000), Some(NOW - idle), Some("claude-opus-5-5")),
            NOW,
            None,
        );
        let ColdCheck::Cold {
            context_tokens,
            idle_ms,
            model,
            estimate_usd,
            not_estimable: None,
            cache_ttl: None,
        } = &check
        else {
            panic!("expected cold: {check:?}");
        };
        assert_eq!((*context_tokens, *idle_ms), (720_000, idle));
        assert_eq!(model.as_deref(), Some("claude-opus-5-5"));
        assert!(
            (estimate_usd.unwrap() - 6.44889).abs() < 1e-9,
            "{estimate_usd:?}"
        );
        assert_eq!(
            refusal(&SeatId("pij-x".into()), &check).unwrap(),
            include_str!("../../testkit/fixtures/golden/cold-wake/refusal.txt")
        );
    }

    /// Plan 160's worked example: a cold 600k Opus seat costs ~$5.39 to wake.
    #[test]
    fn a_wake_is_priced_as_a_cold_write_plus_five_calls() {
        let opus = wake_estimate_usd("claude-opus-5-5", 600_000).unwrap();
        assert_eq!(format!("{opus:.2}"), "5.39");
        // Pinned to consult_costs.py (5-call consult) to the micro-dollar.
        let close = |usd: f64, want: f64| (usd - want).abs() < 1e-6;
        assert!(close(opus, 5.39289), "{opus}");
        let sonnet = wake_estimate_usd("github-copilot/claude-sonnet-5.5", 600_000).unwrap();
        assert!(close(sonnet, 2.93803), "{sonnet}");
        let haiku = wake_estimate_usd("claude-haiku-4-5", 150_000).unwrap();
        assert!(close(haiku, 0.389015), "{haiku}");
        assert_eq!(wake_estimate_usd("some-new-model", 600_000), None);
    }

    #[test]
    fn the_thresholds_are_strict_boundaries() {
        // Unknown TTL at the hour: not refused, but warned about (F214).
        let at_hour = known(
            Some(COLD_CONTEXT_TOKENS + 1),
            Some(NOW - COLD_IDLE_MS),
            None,
        );
        assert!(matches!(
            check(SystemState::Idle, &at_hour, NOW, None),
            ColdCheck::Warn { .. }
        ));
        let at_size = known(
            Some(COLD_CONTEXT_TOKENS),
            Some(NOW - COLD_IDLE_MS - 1),
            None,
        );
        assert_eq!(
            check(SystemState::Idle, &at_size, NOW, None),
            ColdCheck::Clear
        );
        let over_both = known(
            Some(COLD_CONTEXT_TOKENS + 1),
            Some(NOW - COLD_IDLE_MS - 1),
            None,
        );
        assert!(matches!(
            check(SystemState::Idle, &over_both, NOW, None),
            ColdCheck::Cold {
                estimate_usd: None,
                ..
            }
        ));
    }

    /// The ruling's values, pinned literally: a test that compared against the
    /// constants would pass if someone changed them.
    #[test]
    fn the_thresholds_are_the_rulings_400k_and_one_hour() {
        assert_eq!(COLD_CONTEXT_TOKENS, 400_000);
        assert_eq!(COLD_IDLE_MS, 3_600_000);
        let just_over = known(Some(400_001), Some(NOW - 3_600_001), None);
        assert!(matches!(
            check(SystemState::Idle, &just_over, NOW, None),
            ColdCheck::Cold { .. }
        ));
        // Under the hour with an unknown TTL: allowed, with a warning (F214).
        let recent = known(Some(900_000), Some(NOW - 59 * 60 * 1_000), None);
        assert!(matches!(
            check(SystemState::Idle, &recent, NOW, None),
            ColdCheck::Warn { .. }
        ));
        // Jordan 2026-10-13: "long" is 400k, so a 350k seat idle for hours is clear.
        let mid = known(Some(350_000), Some(NOW - 3 * 3_600_000), None);
        assert_eq!(check(SystemState::Idle, &mid, NOW, None), ColdCheck::Clear);
    }

    /// A large seat whose last call carried `ttl`, idle `ago` ms.
    fn large_with_ttl(ago: u64, model: &str, ttl: Fact<CacheTtl>) -> SessionStatusBlock {
        SessionStatusBlock::Known {
            status: Box::new(SeatStatus {
                model: Fact::native(model.to_string()),
                context_used_tokens: Fact::derived(500_000),
                last_call_at_ms: Fact::native(NOW - ago),
                cache_ttl: ttl,
                ..SeatStatus::unknown()
            }),
            cache_state: Fact::Unknown,
            elapsed_ms: 0,
        }
    }

    const MIN: u64 = 60_000;

    /// Option (a), refuse at the TTL.
    const AT_TTL: PastTtl = PastTtl::RefuseAt { ttl_percent: 100 };
    /// Option (c), refuse at 1.5 x the TTL.
    const AT_150: PastTtl = PastTtl::RefuseAt { ttl_percent: 150 };
    /// Option (c), refuse at 2 x the TTL.
    const AT_200: PastTtl = PastTtl::RefuseAt { ttl_percent: 200 };

    fn verdict_kind(check: &ColdCheck) -> &'static str {
        match check {
            ColdCheck::Clear => "clear",
            ColdCheck::Busy => "busy",
            ColdCheck::Cold { .. } => "cold",
            ColdCheck::Warn { .. } => "warn",
            ColdCheck::Unknown { .. } => "unknown",
        }
    }

    /// F214: Copilot records 1800 s for its GPT models. Before, `30m` read as
    /// unknown and the fixed hour let the wake through in silence; now every
    /// value of the switch either warns or refuses past the 30 minutes, and
    /// all of them refuse past the hour.
    #[test]
    fn a_large_gpt_seat_past_its_thirty_minutes_is_warned_or_refused_by_every_rule() {
        let thirty = Fact::native(CacheTtl::from_seconds(1_800).expect("30m"));
        let idle = |ago| large_with_ttl(ago, "gpt-6.1-sol", thirty.clone());
        let minutes = [29, 31, 44, 46, 59, 61];
        for (rule, want) in [
            (
                PastTtl::Warn,
                ["clear", "warn", "warn", "warn", "warn", "cold"],
            ),
            (AT_TTL, ["clear", "cold", "cold", "cold", "cold", "cold"]),
            (AT_150, ["clear", "warn", "warn", "cold", "cold", "cold"]),
            (AT_200, ["clear", "warn", "warn", "warn", "warn", "cold"]),
        ] {
            for (minute, want) in minutes.iter().zip(want) {
                let got = decide(rule, SystemState::Idle, &idle(minute * MIN), NOW, None);
                assert_eq!(
                    verdict_kind(&got),
                    want,
                    "{rule:?} at {minute} min: {got:?}"
                );
            }
        }
        let refused = decide(AT_TTL, SystemState::Idle, &idle(45 * MIN), NOW, None);
        let ColdCheck::Cold { cache_ttl, .. } = &refused else {
            panic!("expected cold: {refused:?}");
        };
        assert_eq!(cache_ttl.as_deref(), Some("30m native"));
        let text = refusal(&SeatId("pij-gpt".into()), &refused).expect("refused");
        assert!(
            text.contains("is cold (500k, idle 45m, cache 30m native)"),
            "{text}"
        );
        let warned = decide(PastTtl::Warn, SystemState::Idle, &idle(45 * MIN), NOW, None);
        assert_eq!(
            warned.receipt_label(false),
            "warn: 500k idle 45m, past its 30m native cache; the wake re-writes it"
        );
    }

    /// F214: the seat's own TTL decides over the table's. A Copilot Claude seat
    /// that recorded a 1-hour cache is warm at 20 min although the table says 5
    /// min; a seat only the table covers is past its TTL at 20 min.
    #[test]
    fn the_seats_own_ttl_decides_over_the_tables() {
        let own = large_with_ttl(
            20 * MIN,
            "claude-opus-5.5",
            Fact::native(CacheTtl::ONE_HOUR),
        );
        let table = large_with_ttl(
            20 * MIN,
            "claude-opus-5.5",
            Fact::Known {
                value: CacheTtl::FIVE_MINUTES,
                basis: Basis::Table { version: 1 },
            },
        );
        for rule in [PastTtl::Warn, AT_TTL, AT_150, AT_200] {
            assert_eq!(
                decide(rule, SystemState::Idle, &own, NOW, None),
                ColdCheck::Clear,
                "{rule:?}"
            );
        }
        let ColdCheck::Cold { cache_ttl, .. } =
            decide(AT_200, SystemState::Idle, &table, NOW, None)
        else {
            panic!("a table-only 5m seat idle 20 min is past 2 x its TTL");
        };
        assert_eq!(cache_ttl.as_deref(), Some("5m table@v1"));
    }

    /// F214: a large seat with no TTL from any rung is warned about once idle
    /// past the shortest TTL; never refused before the hour, never silent.
    #[test]
    fn an_unknown_ttl_on_a_large_idle_seat_warns() {
        let unknown_ttl = |ago| large_with_ttl(ago, "some-new-model", Fact::Unknown);
        for rule in [PastTtl::Warn, AT_TTL, AT_150, AT_200] {
            assert_eq!(
                decide(
                    rule,
                    SystemState::Idle,
                    &unknown_ttl(UNKNOWN_TTL_WARN_MS),
                    NOW,
                    None
                ),
                ColdCheck::Clear,
                "{rule:?}: not past the shortest TTL"
            );
            let warned = decide(rule, SystemState::Idle, &unknown_ttl(10 * MIN), NOW, None);
            assert_eq!(
                warned.receipt_label(false),
                "warn: cache TTL unknown; 500k idle 10m may be cold",
                "{rule:?}"
            );
            assert!(refusal(&SeatId("pij-x".into()), &warned).is_none());
        }
        // Small seats are never warned about: "large" is COLD_CONTEXT_TOKENS.
        assert_eq!(
            check(
                SystemState::Idle,
                &known(Some(COLD_CONTEXT_TOKENS), Some(NOW - 30 * MIN), None),
                NOW,
                None
            ),
            ColdCheck::Clear
        );
        let size = seat_size(SystemState::Idle, &unknown_ttl(10 * MIN), NOW, None);
        assert!(!size.cold_wake.would_refuse);
        assert_eq!(size_columns(&size)[3], "⚠");
        assert_eq!(
            size_lines(&unknown_ttl(10 * MIN), &size)[1],
            "⚠ cold-wake guard: a normal send goes through with a warning: cache TTL unknown; 500k idle 10m may be cold"
        );
    }

    /// A last call stamped after the daemon's clock (skew) is recent, never
    /// "idle for centuries".
    #[test]
    fn a_future_last_call_is_not_cold() {
        let skewed = known(Some(900_000), Some(NOW + 60 * 60 * 1_000), None);
        assert_eq!(
            check(SystemState::Idle, &skewed, NOW, None),
            ColdCheck::Clear
        );
    }

    #[test]
    fn a_working_seat_is_never_cold() {
        let cold = known(Some(900_000), Some(0), Some("claude-opus-5-5"));
        assert_eq!(
            check(SystemState::Working, &cold, NOW, None),
            ColdCheck::Busy
        );
    }

    #[test]
    fn anything_unknown_allows_and_says_why() {
        for block in [
            SessionStatusBlock::Unbound,
            SessionStatusBlock::Unsupported {
                harness: Harness::Omp,
            },
            SessionStatusBlock::NotFound { detail: "x".into() },
            SessionStatusBlock::Failed {
                error: "boom".into(),
            },
            known(None, Some(0), None),
            known(Some(900_000), None, None),
        ] {
            let check = check(SystemState::Idle, &block, NOW, None);
            assert!(
                matches!(check, ColdCheck::Unknown { .. }),
                "{block:?} -> {check:?}"
            );
            assert!(refusal(&SeatId("pij-x".into()), &check).is_none());
            assert!(check.receipt_label(false).starts_with("unknown: "));
        }
    }

    #[test]
    fn an_unpriced_model_still_refuses_but_says_the_price_is_unknown() {
        let check = check(
            SystemState::Idle,
            &known(Some(450_000), Some(0), Some("some-new-model")),
            NOW,
            None,
        );
        let text = refusal(&SeatId("pij-y".into()), &check).unwrap();
        assert!(text.contains("price unknown"), "{text}");
        assert!(text.contains("is cold (450k, idle 10h)"), "{text}");
        assert!(
            text.contains("--fyi") && text.contains("--force --reason"),
            "{text}"
        );
    }

    /// Plan 169 R7: a router seat still refuses on size and idle, but its wake
    /// has no single model to price, even when its last call ran on a priced one.
    #[test]
    fn a_router_seat_refuses_without_a_price_and_says_why() {
        let served_by_opus = known(Some(720_000), Some(0), Some("claude-opus-5-5"));
        for (block, seat_model) in [
            (&served_by_opus, Some("hydrafusion-max")),
            (&known(Some(720_000), Some(0), Some("hydrafusion")), None),
        ] {
            let verdict = check(SystemState::Idle, block, NOW, seat_model);
            let ColdCheck::Cold {
                estimate_usd,
                not_estimable,
                ..
            } = &verdict
            else {
                panic!("a large idle router seat is still cold: {verdict:?}");
            };
            assert_eq!(*estimate_usd, None);
            assert_eq!(not_estimable.as_deref(), Some(ROUTER_NOT_ESTIMABLE));
            let text = refusal(&SeatId("pij-router".into()), &verdict).unwrap();
            assert!(
                text.contains("? (not estimable: router, per-call models)"),
                "{text}"
            );
            assert!(!text.contains("~$"), "{text}");
            let size = seat_size(SystemState::Idle, block, NOW, seat_model);
            assert!(size.cold_wake.would_refuse);
            assert_eq!(size.cold_wake.estimate_usd, None);
            assert!(size_lines(block, &size)[1].contains(ROUTER_NOT_ESTIMABLE));
        }
        let ColdCheck::Cold { estimate_usd, .. } = check(
            SystemState::Idle,
            &served_by_opus,
            NOW,
            Some("claude-opus-5-5"),
        ) else {
            panic!("an ordinary large idle seat is cold");
        };
        assert_eq!(estimate_usd, wake_estimate_usd("claude-opus-5-5", 720_000));
    }
}
