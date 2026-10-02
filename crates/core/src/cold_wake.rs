//! The cold-wake guard (plan 157 phase 2, ruling #446).
//!
//! Waking a seat whose prompt cache has expired re-writes its whole context at
//! cache-write prices. On a large seat that costs dollars per message, and the
//! 2026-09-28 usage blowout was made of exactly these wakes. So `pij send`
//! refuses a message to a seat that is cold (large, idle past the cache TTL,
//! and not working), unless the sender forces it with a reason.
//!
//! **This is a brake, not a policy.** It can only refuse. It never changes what
//! is delivered or how. Every fact it cannot establish (unsupported harness,
//! source failure, no transcript, unknown context or last-call time) allows the
//! send and says so. Removing the guard delivers the same messages or more.

use serde::{Deserialize, Serialize};

use crate::model::{SeatId, SystemState};
use crate::session_status::{CacheState, CacheTtl, Fact, SessionStatusBlock};

/// A seat holding more than this many context tokens is large enough to guard.
pub const COLD_CONTEXT_TOKENS: u64 = 300_000;

/// A seat whose last API call is older than this has lost its prompt cache.
pub const COLD_IDLE_MS: u64 = 60 * 60 * 1_000;

/// The refusal's stable code.
pub const COLD_WAKE_CODE: &str = "E-RS-COLD-WAKE";

/// The spine kind recording every forced cold wake: the audit trail.
pub const COLD_WAKE_FORCED_KIND: &str = "send.cold-wake-forced";

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
    /// Large and idle past the TTL: refuse unless forced.
    #[serde(rename_all = "camelCase")]
    Cold {
        /// Tokens the wake would re-write.
        context_tokens: u64,
        /// Milliseconds since the seat's last API call.
        idle_ms: u64,
        /// The seat's current model, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Estimated list cost of the wake ([`wake_estimate_usd`]), when the
        /// model is priced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimate_usd: Option<f64>,
    },
    /// A fact the rule needs is unknown, so the send is allowed.
    Unknown {
        /// Which fact, or which failure.
        why: String,
    },
}

impl ColdCheck {
    /// The receipt's `cold_check` line: `clear`, `busy`, `forced` or `unknown: <why>`.
    pub fn receipt_label(&self, forced: bool) -> String {
        match self {
            Self::Clear => "clear".to_string(),
            Self::Busy => "busy".to_string(),
            Self::Cold { .. } if forced => "forced".to_string(),
            Self::Cold { .. } => "cold".to_string(),
            Self::Unknown { why } => format!("unknown: {why}"),
        }
    }
}

/// Decide one recipient. `state` is the daemon's own busy/idle observation;
/// `status` is the `pij state` session block; `now_ms` is the daemon clock.
pub fn check(state: SystemState, status: &SessionStatusBlock, now_ms: u64) -> ColdCheck {
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
    if idle_ms <= COLD_IDLE_MS {
        return ColdCheck::Clear;
    }
    let model = match &status.model {
        Fact::Known { value, .. } => Some(value.clone()),
        Fact::Unknown => None,
    };
    let estimate_usd = model
        .as_deref()
        .and_then(|model| wake_estimate_usd(model, context_tokens));
    ColdCheck::Cold {
        context_tokens,
        idle_ms,
        model,
        estimate_usd,
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
/// error), and an unknown last call or unknown TTL is not warm.
pub fn is_warm(status: &SessionStatusBlock, now_ms: u64) -> bool {
    let SessionStatusBlock::Known { status, .. } = status else {
        return false;
    };
    let (Some(&last_call), Some(&ttl)) = (status.last_call_at_ms.value(), status.cache_ttl.value())
    else {
        return false;
    };
    let lifetime_ms = ttl.as_ms().min(COLD_IDLE_MS);
    now_ms < last_call.saturating_add(lifetime_ms)
}

/// The refusal a sender sees: the seat's size and idle time, then every option
/// the sender has, each priced. One row per option, so a new one is a new row.
pub fn refusal(recipient: &SeatId, check: &ColdCheck) -> Option<String> {
    let ColdCheck::Cold {
        context_tokens,
        idle_ms,
        estimate_usd,
        ..
    } = check
    else {
        return None;
    };
    let wake =
        estimate_usd.map_or_else(|| "price unknown".to_string(), |usd| format!("~${usd:.2}"));
    let options = [
        ("--fyi", "hold for its next turn", "$0 now".to_string()),
        ("--force --reason \"…\"", "wake on its current model", wake),
    ];
    let mut text = format!(
        "{COLD_WAKE_CODE}: ❄ {recipient} is cold ({}, idle {}). Options:",
        human_tokens(*context_tokens),
        human_duration(*idle_ms),
    );
    for (flag, effect, price) in options {
        text.push_str(&format!("\n  {flag:<22} {effect:<31} {price}"));
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
}

/// Derive a seat's [`SeatSize`]. The refusal verdict is [`check`] itself, so
/// a ❄ row is exactly a seat the guard would refuse now.
pub fn seat_size(state: SystemState, status: &SessionStatusBlock, now_ms: u64) -> SeatSize {
    let cold_wake = match check(state, status, now_ms) {
        ColdCheck::Cold { estimate_usd, .. } => ColdWakeView {
            would_refuse: true,
            estimate_usd,
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

/// The list columns CTX, IDLE and CACHE, `?` where unknown, and the ❄ marker.
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

/// `pij state`'s size line and, when the guard would refuse, its ❄ line:
/// `context 375k / 1M · last call 14m ago · cache 5m (cold 9m) · 3 compactions`.
/// Nothing when the facts could not be read (the `session:` line says why).
pub fn size_lines(status: &SessionStatusBlock, size: &SeatSize) -> Vec<String> {
    let SessionStatusBlock::Known { status, .. } = status else {
        return Vec::new();
    };
    let known = |value: Option<String>| value.unwrap_or_else(|| "?".to_string());
    let ttl = status.cache_ttl.value().map(|ttl| match ttl {
        CacheTtl::FiveMinutes => "5m",
        CacheTtl::OneHour => "1h",
    });
    let cache = match (ttl, &size.cache_state) {
        (Some(ttl), Some(state)) => format!("{ttl} ({})", cache_text(state)),
        (Some(ttl), None) => ttl.to_string(),
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
        let price = size
            .cold_wake
            .estimate_usd
            .map_or_else(|| "price unknown".to_string(), |usd| format!("~${usd:.2}"));
        lines.push(format!(
            "❄ cold-wake guard: a normal send is refused; waking it costs {price} (--fyi holds it for $0 now)"
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
            cache_ttl: Fact::derived(CacheTtl::OneHour),
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
        let cold = seat_size(SystemState::Idle, &full(two_hours), NOW);
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
            let size = seat_size(state, &full(ago), NOW);
            assert!(!size.cold_wake.would_refuse, "{state:?} {ago}");
            assert_eq!(size_columns(&size)[3], "");
        }
        let warm = seat_size(SystemState::Idle, &full(60_000), NOW);
        assert_eq!(
            size_columns(&warm),
            ["720k", "1m", "warm", ""].map(String::from)
        );
        // Nothing known: every column is `?`, never a guess.
        let unknown = seat_size(SystemState::Idle, &SessionStatusBlock::Unbound, NOW);
        assert_eq!(
            size_columns(&unknown),
            ["?", "?", "?", ""].map(String::from)
        );
    }

    #[test]
    fn the_state_lines_name_size_cache_and_the_guard() {
        let block = full(2 * COLD_IDLE_MS);
        let size = seat_size(SystemState::Idle, &block, NOW);
        assert_eq!(
            size_lines(&block, &size),
            [
                "context 720k / 1M · last call 2h ago · cache 1h (cold 1h) · 3 compactions",
                "❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi holds it for $0 now)",
            ]
            .map(String::from)
        );
        let warm = full(60_000);
        assert_eq!(
            size_lines(&warm, &seat_size(SystemState::Idle, &warm, NOW)),
            [
                "context 720k / 1M · last call 1m ago · cache 1h (warm) · 3 compactions"
                    .to_string()
            ]
        );
        assert!(size_lines(&SessionStatusBlock::Unbound, &SeatSize::default()).is_empty());
        assert_eq!(human_tokens(1_000_000), "1M");
        assert_eq!(human_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn warm_needs_a_known_last_call_inside_the_real_cache_lifetime() {
        use crate::session_status::CacheTtl::{FiveMinutes, OneHour};
        let at = |ago: u64, ttl| SessionStatusBlock::Known {
            status: Box::new(SeatStatus {
                last_call_at_ms: Fact::native(NOW - ago),
                cache_ttl: Fact::derived(ttl),
                ..SeatStatus::unknown()
            }),
            cache_state: Fact::Unknown,
            elapsed_ms: 0,
        };
        let five = FiveMinutes.as_ms();
        assert!(is_warm(&at(five - 1, FiveMinutes), NOW));
        assert!(
            !is_warm(&at(five, FiveMinutes), NOW),
            "expired at its lifetime"
        );
        assert!(is_warm(&at(COLD_IDLE_MS - 1, OneHour), NOW));
        assert!(!is_warm(&at(COLD_IDLE_MS, OneHour), NOW));
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
        );
        let ColdCheck::Cold {
            context_tokens,
            idle_ms,
            model,
            estimate_usd,
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
        let at_ttl = known(
            Some(COLD_CONTEXT_TOKENS + 1),
            Some(NOW - COLD_IDLE_MS),
            None,
        );
        assert_eq!(check(SystemState::Idle, &at_ttl, NOW), ColdCheck::Clear);
        let at_size = known(
            Some(COLD_CONTEXT_TOKENS),
            Some(NOW - COLD_IDLE_MS - 1),
            None,
        );
        assert_eq!(check(SystemState::Idle, &at_size, NOW), ColdCheck::Clear);
        let over_both = known(
            Some(COLD_CONTEXT_TOKENS + 1),
            Some(NOW - COLD_IDLE_MS - 1),
            None,
        );
        assert!(matches!(
            check(SystemState::Idle, &over_both, NOW),
            ColdCheck::Cold {
                estimate_usd: None,
                ..
            }
        ));
    }

    /// The ruling's values, pinned literally: a test that compared against the
    /// constants would pass if someone changed them.
    #[test]
    fn the_thresholds_are_the_rulings_300k_and_one_hour() {
        assert_eq!(COLD_CONTEXT_TOKENS, 300_000);
        assert_eq!(COLD_IDLE_MS, 3_600_000);
        let just_over = known(Some(300_001), Some(NOW - 3_600_001), None);
        assert!(matches!(
            check(SystemState::Idle, &just_over, NOW),
            ColdCheck::Cold { .. }
        ));
        let recent = known(Some(900_000), Some(NOW - 59 * 60 * 1_000), None);
        assert_eq!(check(SystemState::Idle, &recent, NOW), ColdCheck::Clear);
    }

    /// A last call stamped after the daemon's clock (skew) is recent, never
    /// "idle for centuries".
    #[test]
    fn a_future_last_call_is_not_cold() {
        let skewed = known(Some(900_000), Some(NOW + 60 * 60 * 1_000), None);
        assert_eq!(check(SystemState::Idle, &skewed, NOW), ColdCheck::Clear);
    }

    #[test]
    fn a_working_seat_is_never_cold() {
        let cold = known(Some(900_000), Some(0), Some("claude-opus-5-5"));
        assert_eq!(check(SystemState::Working, &cold, NOW), ColdCheck::Busy);
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
            let check = check(SystemState::Idle, &block, NOW);
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
            &known(Some(400_000), Some(0), Some("some-new-model")),
            NOW,
        );
        let text = refusal(&SeatId("pij-y".into()), &check).unwrap();
        assert!(text.contains("price unknown"), "{text}");
        assert!(text.contains("is cold (400k, idle 10h)"), "{text}");
        assert!(
            text.contains("--fyi") && text.contains("--force --reason"),
            "{text}"
        );
    }
}
