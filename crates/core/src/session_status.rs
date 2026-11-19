//! Per-seat session facts read from the harness's own transcript (plan 157).
//!
//! The daemon prices each `pij send` by what waking the recipient costs:
//! how much context it holds and whether its prompt cache is still warm. A
//! cold wake costs about 40× a warm one (2026-09-28 usage RCA). Those facts come
//! from [`crate::ports::SessionStatusPort`], and this module is the pij-owned
//! shape they arrive in. Adapter vocabulary never crosses this line.
//!
//! **Every fact carries its basis or says it is unknown.** A harness that does
//! not record a fact reports [`Fact::Unknown`], never `0`, because `0` is a
//! claim ("this seat has no context") that a pricing guard would act on.
//!
//! **Busy/idle is not here.** Whether a seat is mid-turn is the daemon's own
//! observation (`SeatDescriptor::state`), not a transcript inference.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::{Harness, SeatId};

/// The version of [`SeatStatus`] on the wire. Bump it when a field changes meaning.
///
/// v2 (F214): `cacheTtlSeconds` replaced `cacheTtl`; the TTL is a duration,
/// no longer one of two buckets, and its basis may be `proxy` or `table@vN`.
pub const SEAT_STATUS_VERSION: u32 = 2;

/// Where a known fact came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Recorded by the harness itself.
    Native,
    /// Computed from native records, such as a sum or a latest-of.
    Derived,
    /// Looked up in a versioned table, such as a model's context window.
    Table {
        /// The table's version.
        version: u32,
    },
    /// Taken from file mtime because no native timestamp exists.
    MtimeFallback,
    /// Taken from the seat's route through the pij cache proxy (F214).
    Proxy,
}

impl Basis {
    /// The basis as the wire spells it, which the human lines show too:
    /// `native`, `derived`, `table@v1`, `mtime-fallback` or `proxy`.
    pub fn to_wire(self) -> String {
        match self {
            Self::Native => "native".to_string(),
            Self::Derived => "derived".to_string(),
            Self::Table { version } => format!("table@v{version}"),
            Self::MtimeFallback => "mtime-fallback".to_string(),
            Self::Proxy => "proxy".to_string(),
        }
    }

    fn from_wire(text: &str) -> Option<Self> {
        match text {
            "native" => Some(Self::Native),
            "derived" => Some(Self::Derived),
            "mtime-fallback" => Some(Self::MtimeFallback),
            "proxy" => Some(Self::Proxy),
            other => other
                .strip_prefix("table@v")
                .and_then(|version| version.parse().ok())
                .map(|version| Self::Table { version }),
        }
    }
}

/// The spelling of [`Fact::Unknown`]'s basis on the wire.
const UNKNOWN_BASIS: &str = "unknown";

/// One session fact, known with its basis or explicitly unknown.
///
/// Wire shape: `{"value": V, "basis": "native"}` or `{"basis": "unknown"}`.
/// There is no `null` value and no default, so a missing fact can't read as zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fact<T> {
    /// The harness recorded it, or it was derived from what it recorded.
    Known {
        /// The fact.
        value: T,
        /// Where it came from.
        basis: Basis,
    },
    /// The harness did not record it.
    Unknown,
}

impl<T> Fact<T> {
    /// A fact the harness recorded itself.
    pub const fn native(value: T) -> Self {
        Self::Known {
            value,
            basis: Basis::Native,
        }
    }

    /// A fact computed from native records.
    pub const fn derived(value: T) -> Self {
        Self::Known {
            value,
            basis: Basis::Derived,
        }
    }

    /// The value when known.
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Known { value, .. } => Some(value),
            Self::Unknown => None,
        }
    }

    /// The basis when known.
    pub const fn basis(&self) -> Option<Basis> {
        match self {
            Self::Known { basis, .. } => Some(*basis),
            Self::Unknown => None,
        }
    }
}

#[derive(Serialize)]
struct FactOut<'a, T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a T>,
    basis: String,
}

#[derive(Deserialize)]
struct FactIn<T> {
    value: Option<T>,
    basis: String,
}

impl<T: Serialize> Serialize for Fact<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Self::Known { value, basis } => FactOut {
                value: Some(value),
                basis: basis.to_wire(),
            },
            Self::Unknown => FactOut {
                value: None,
                basis: UNKNOWN_BASIS.to_string(),
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Fact<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FactIn::<T>::deserialize(deserializer)?;
        match (wire.value, wire.basis.as_str()) {
            (None, UNKNOWN_BASIS) => Ok(Self::Unknown),
            (Some(_), UNKNOWN_BASIS) => Err(D::Error::custom("an unknown fact carries no value")),
            (None, basis) => Err(D::Error::custom(format!(
                "a `{basis}` fact must carry a value"
            ))),
            (Some(value), basis) => Basis::from_wire(basis)
                .map(|basis| Self::Known { value, basis })
                .ok_or_else(|| D::Error::custom(format!("unknown fact basis `{basis}`"))),
        }
    }
}

/// How long the provider keeps the last call's prompt cache (F214): a real
/// duration, never a bucket. Claude writes 5 minutes or 1 hour, Copilot's GPT
/// models record 30 minutes, and the next provider may record anything else.
/// On the wire it is whole seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CacheTtl {
    seconds: u64,
}

impl CacheTtl {
    /// Anthropic's default ephemeral cache.
    pub const FIVE_MINUTES: Self = Self { seconds: 300 };
    /// Anthropic's extended cache, and the pij cache proxy's main chain.
    pub const ONE_HOUR: Self = Self { seconds: 3_600 };

    /// A TTL of `seconds`; `None` for zero, which is no cache at all.
    pub const fn from_seconds(seconds: u64) -> Option<Self> {
        if seconds == 0 {
            None
        } else {
            Some(Self { seconds })
        }
    }

    /// Parse Unisphere's label: `5m`, `30m`, `1h`, `24h` or `90s`.
    pub fn from_label(label: &str) -> Option<Self> {
        let (number, scale) = if let Some(number) = label.strip_suffix('s') {
            (number, 1)
        } else if let Some(number) = label.strip_suffix('m') {
            (number, 60)
        } else {
            (label.strip_suffix('h')?, 3_600)
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Self::from_seconds(number.parse::<u64>().ok()?.checked_mul(scale)?)
    }

    /// The TTL in seconds.
    pub const fn seconds(self) -> u64 {
        self.seconds
    }

    /// The TTL in milliseconds.
    pub const fn as_ms(self) -> u64 {
        self.seconds.saturating_mul(1_000)
    }

    /// `5m`, `30m`, `1h`, `90s`: the largest whole unit.
    pub fn label(self) -> String {
        match self.seconds {
            s if s % 3_600 == 0 => format!("{}h", s / 3_600),
            s if s % 60 == 0 => format!("{}m", s / 60),
            s => format!("{s}s"),
        }
    }
}

/// The version of [`CACHE_TTL_TABLE`]; a TTL taken from it says `table@vN`.
pub const CACHE_TTL_TABLE_VERSION: u32 = 1;

/// The last rung of a seat's cache TTL (F214): what each harness and model
/// family writes, for a seat whose own records carry no TTL. Rows: harness, the
/// provider the seat must route through (`None`: the harness's own), the model
/// family prefix, the TTL in seconds. Taken from the F214 audit of 2026-11-12
/// to 2026-11-19: Claude Code wrote 1 h on every call; plain Copilot Claude
/// recorded 300 s on most calls and OMP through github-copilot wrote 5 m, so
/// the shorter TTL stands; Copilot recorded 1800 s for every GPT model.
pub const CACHE_TTL_TABLE: &[(Harness, Option<&str>, &str, u64)] = &[
    (Harness::Claude, None, "claude-", 3_600),
    (Harness::Copilot, None, "claude-", 300),
    (Harness::Copilot, None, "gpt-", 1_800),
    (Harness::Omp, Some("github-copilot"), "claude-", 300),
    (Harness::Omp, Some("github-copilot"), "gpt-", 1_800),
];

/// What a seat's descriptor says about its cache, for the TTL rungs below the
/// seat's own records.
#[derive(Clone, Copy, Debug)]
pub struct TtlSeat<'a> {
    /// The seat's harness.
    pub harness: Harness,
    /// The seat's model calls go through the pij cache proxy.
    pub proxied: bool,
    /// The model the seat was bound with.
    pub model: Option<&'a str>,
    /// The provider the seat was bound with (OMP names one).
    pub provider: Option<&'a str>,
}

/// Fill an unknown cache TTL from the rungs below the seat's own records, in
/// order (F214): the seat's records (native, or derived from its native 1 h/5 m
/// write split) are kept as they are; then the pij cache proxy (a proxied
/// Claude seat's main chain writes 1 h); then [`CACHE_TTL_TABLE`]. Every
/// filled TTL says which rung it came from in its basis, and a seat no rung
/// covers stays unknown.
pub fn fill_cache_ttl(status: &mut SeatStatus, seat: &TtlSeat<'_>) {
    if status.cache_ttl.value().is_some() {
        return;
    }
    let Some(model) = status.model.value().map(String::as_str).or(seat.model) else {
        return;
    };
    let (prefix, name) = match model.split_once('/') {
        Some((prefix, name)) => (Some(prefix), name),
        None => (None, model),
    };
    let provider = prefix.or(seat.provider);
    if seat.proxied && name.starts_with("claude-") {
        status.cache_ttl = Fact::Known {
            value: CacheTtl::ONE_HOUR,
            basis: Basis::Proxy,
        };
        return;
    }
    let row = CACHE_TTL_TABLE.iter().find(|(harness, route, family, _)| {
        *harness == seat.harness
            && route.is_none_or(|route| provider == Some(route))
            && name.starts_with(family)
    });
    if let Some(ttl) = row.and_then(|(.., seconds)| CacheTtl::from_seconds(*seconds)) {
        status.cache_ttl = Fact::Known {
            value: ttl,
            basis: Basis::Table {
                version: CACHE_TTL_TABLE_VERSION,
            },
        };
    }
}

/// One seat's session facts, as the port reports them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatStatus {
    /// [`SEAT_STATUS_VERSION`] at the time the port produced this.
    pub version: u32,
    /// The latest main-chain call's model.
    pub model: Fact<String>,
    /// Tokens the next call will re-read: the last call's input plus cache read and write.
    pub context_used_tokens: Fact<u64>,
    /// The model's context window.
    pub context_window_tokens: Fact<u64>,
    /// When the last main-chain call was made, in epoch ms.
    pub last_call_at_ms: Fact<u64>,
    /// The last call's uncached input tokens.
    pub last_call_input_tokens: Fact<u64>,
    /// The last call's cache-read tokens.
    pub last_call_cache_read_tokens: Fact<u64>,
    /// The last call's 5-minute cache-write tokens.
    pub last_call_cache_write_5m_tokens: Fact<u64>,
    /// The last call's 1-hour cache-write tokens.
    pub last_call_cache_write_1h_tokens: Fact<u64>,
    /// The cache TTL the last call wrote: native when the harness recorded it,
    /// derived from its 1 h/5 m write split, else from [`fill_cache_ttl`]'s
    /// rungs, each with its basis. A card from a v1 daemon (mid-upgrade, or
    /// a paired machine) has no such field, so it reads as unknown, never as
    /// a refused card.
    #[serde(rename = "cacheTtlSeconds", default = "unknown_ttl")]
    pub cache_ttl: Fact<CacheTtl>,
    /// How many times the session was compacted.
    pub compactions: Fact<u64>,
    /// Why the source re-read the transcript from the start, when it did.
    ///
    /// `None` means it continued from its previous position, which is the warm path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
}

impl SeatStatus {
    /// A status in which every fact is unknown. Adapters fill in what they know.
    pub const fn unknown() -> Self {
        Self {
            version: SEAT_STATUS_VERSION,
            model: Fact::Unknown,
            context_used_tokens: Fact::Unknown,
            context_window_tokens: Fact::Unknown,
            last_call_at_ms: Fact::Unknown,
            last_call_input_tokens: Fact::Unknown,
            last_call_cache_read_tokens: Fact::Unknown,
            last_call_cache_write_5m_tokens: Fact::Unknown,
            last_call_cache_write_1h_tokens: Fact::Unknown,
            cache_ttl: Fact::Unknown,
            compactions: Fact::Unknown,
            reset: None,
        }
    }
}

/// A v1 card's missing `cacheTtlSeconds`: unknown.
const fn unknown_ttl() -> Fact<CacheTtl> {
    Fact::Unknown
}

/// Whether the recipient's prompt cache is still warm at `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CacheState {
    /// A send now reuses the cache.
    Warm {
        /// Milliseconds until the cache expires.
        #[serde(rename = "expiresInMs")]
        expires_in_ms: u64,
    },
    /// The cache has expired, so a send now re-writes the whole context.
    Cold {
        /// Milliseconds since the cache expired.
        #[serde(rename = "expiredForMs")]
        expired_for_ms: u64,
    },
}

/// Derive the cache state at `now_ms` from the last call time and TTL.
///
/// Unknown when either input is unknown, because guessing a TTL would turn a
/// cold seat warm, which is the expensive error. A TTL from a lower rung
/// ([`fill_cache_ttl`]) is no guess: it carries its basis, and the views show
/// it. A last call stamped after `now_ms` (clock skew) counts as just made.
pub fn cache_state(status: &SeatStatus, now_ms: u64) -> Fact<CacheState> {
    let (Some(&last_call), Some(&ttl)) = (status.last_call_at_ms.value(), status.cache_ttl.value())
    else {
        return Fact::Unknown;
    };
    let expires_at = last_call.saturating_add(ttl.as_ms());
    Fact::derived(if now_ms < expires_at {
        CacheState::Warm {
            expires_in_ms: expires_at - now_ms,
        }
    } else {
        CacheState::Cold {
            expired_for_ms: now_ms - expires_at,
        }
    })
}

/// The session a port reads for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    /// The seat. Sources key their per-seat read position by it.
    pub seat: SeatId,
    /// The seat's harness.
    pub harness: Harness,
    /// The harness-native session id bound to the seat.
    pub session: String,
}

/// What a [`crate::ports::SessionStatusPort`] found for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatusReply {
    /// The session was read.
    Status(SeatStatus),
    /// The source can't read this harness's sessions.
    Unsupported,
    /// The source supports the harness but found no transcript for the session.
    NotFound {
        /// What was looked for, and where.
        detail: String,
    },
}

/// The `sessionStatus` block on `pij state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum SessionStatusBlock {
    /// The session was read.
    #[serde(rename_all = "camelCase")]
    Known {
        /// The facts.
        status: Box<SeatStatus>,
        /// The cache state at the daemon's clock.
        cache_state: Fact<CacheState>,
        /// How long the source took, in ms.
        elapsed_ms: u64,
    },
    /// The seat has no harness session bound, so there is nothing to read.
    Unbound,
    /// The source can't read this harness's sessions.
    Unsupported {
        /// The seat's harness.
        harness: Harness,
    },
    /// No transcript was found for the bound session.
    NotFound {
        /// What was looked for, and where.
        detail: String,
    },
    /// The source failed. The rest of the state card is still valid.
    Failed {
        /// The source's error.
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warm_capable(last_call: u64, ttl: CacheTtl) -> SeatStatus {
        SeatStatus {
            last_call_at_ms: Fact::native(last_call),
            cache_ttl: Fact::derived(ttl),
            ..SeatStatus::unknown()
        }
    }

    #[test]
    fn cache_is_warm_until_the_ttl_elapses_then_cold() {
        let status = warm_capable(1_000, CacheTtl::FIVE_MINUTES);
        assert_eq!(
            cache_state(&status, 1_000 + 299_999),
            Fact::derived(CacheState::Warm { expires_in_ms: 1 })
        );
        assert_eq!(
            cache_state(&status, 1_000 + 300_000),
            Fact::derived(CacheState::Cold { expired_for_ms: 0 })
        );
        let hour = warm_capable(0, CacheTtl::ONE_HOUR);
        assert_eq!(
            cache_state(&hour, 600_000),
            Fact::derived(CacheState::Warm {
                expires_in_ms: 3_000_000
            })
        );
        // F214: a GPT seat's 30-minute cache is cold 30 minutes after its last
        // call, never "unknown" because it is neither 5 minutes nor an hour.
        let gpt = warm_capable(0, CacheTtl::from_label("30m").expect("30m"));
        assert_eq!(
            cache_state(&gpt, 45 * 60_000),
            Fact::derived(CacheState::Cold {
                expired_for_ms: 15 * 60_000
            })
        );
    }

    #[test]
    fn a_ttl_label_parses_to_its_duration_and_prints_back() {
        for (label, seconds) in [
            ("5m", 300),
            ("30m", 1_800),
            ("1h", 3_600),
            ("24h", 86_400),
            ("90s", 90),
        ] {
            let ttl = CacheTtl::from_label(label).expect(label);
            assert_eq!(ttl.seconds(), seconds, "{label}");
            assert_eq!(ttl.label(), label);
        }
        for bad in [
            "",
            "m",
            "0m",
            "5d",
            "-5m",
            "+5m",
            "5é",
            "five-minutes",
            "1.5h",
        ] {
            assert_eq!(CacheTtl::from_label(bad), None, "{bad}");
        }
        assert_eq!(
            serde_json::to_value(Fact::native(CacheTtl::FIVE_MINUTES)).expect("wire"),
            serde_json::json!({"value": 300, "basis": "native"})
        );
    }

    fn seat<'a>(harness: Harness, model: &'a str) -> TtlSeat<'a> {
        TtlSeat {
            harness,
            proxied: false,
            model: Some(model),
            provider: None,
        }
    }

    fn filled(status: SeatStatus, seat: &TtlSeat<'_>) -> Fact<CacheTtl> {
        let mut status = status;
        fill_cache_ttl(&mut status, seat);
        status.cache_ttl
    }

    /// F214's source order: the seat's own record, then the proxy, then the
    /// table. A rung never overrides one above it.
    #[test]
    fn the_seats_own_ttl_wins_over_the_proxy_and_the_table() {
        let table = Basis::Table {
            version: CACHE_TTL_TABLE_VERSION,
        };
        let recorded = SeatStatus {
            cache_ttl: Fact::native(CacheTtl::ONE_HOUR),
            ..SeatStatus::unknown()
        };
        // The table says 5 minutes for plain Copilot Claude; the record says 1 h.
        assert_eq!(
            filled(recorded, &seat(Harness::Copilot, "claude-opus-5.5")),
            Fact::native(CacheTtl::ONE_HOUR)
        );
        let silent = SeatStatus {
            model: Fact::native("claude-opus-5.5".to_string()),
            ..SeatStatus::unknown()
        };
        let proxied = TtlSeat {
            proxied: true,
            ..seat(Harness::Copilot, "claude-opus-5.5")
        };
        assert_eq!(
            filled(silent.clone(), &proxied),
            Fact::Known {
                value: CacheTtl::ONE_HOUR,
                basis: Basis::Proxy
            }
        );
        assert_eq!(
            filled(silent, &seat(Harness::Copilot, "claude-opus-5.5")),
            Fact::Known {
                value: CacheTtl::FIVE_MINUTES,
                basis: table
            }
        );
        // A proxied GPT seat is not proxied at all (the proxy serves Claude only).
        let gpt = TtlSeat {
            proxied: true,
            ..seat(Harness::Copilot, "gpt-6.1-sol")
        };
        assert_eq!(
            filled(SeatStatus::unknown(), &gpt),
            Fact::Known {
                value: CacheTtl::from_seconds(1_800).expect("30m"),
                basis: table
            }
        );
    }

    #[test]
    fn the_table_matches_the_served_model_and_the_route() {
        let ttl = |status: SeatStatus, seat: TtlSeat<'_>| {
            filled(status, &seat).value().map(|ttl| ttl.seconds())
        };
        let served = |model: &str| SeatStatus {
            model: Fact::native(model.to_string()),
            ..SeatStatus::unknown()
        };
        // OMP names its provider in the served model, or on the seat.
        assert_eq!(
            ttl(
                served("github-copilot/gpt-6.1-sol"),
                seat(Harness::Omp, "x")
            ),
            Some(1_800)
        );
        let on_seat = TtlSeat {
            provider: Some("github-copilot"),
            ..seat(Harness::Omp, "claude-opus-5.5")
        };
        assert_eq!(ttl(SeatStatus::unknown(), on_seat), Some(300));
        // OMP through another provider, Codex, or an unknown model: no row.
        assert_eq!(
            ttl(served("anthropic/claude-opus-5-5"), seat(Harness::Omp, "x")),
            None
        );
        assert_eq!(
            ttl(SeatStatus::unknown(), seat(Harness::Codex, "gpt-6.1")),
            None
        );
        assert_eq!(
            ttl(
                SeatStatus::unknown(),
                seat(Harness::Copilot, "gemini-3.6-flash")
            ),
            None
        );
        assert_eq!(
            ttl(
                SeatStatus::unknown(),
                seat(Harness::Claude, "claude-opus-5-5")
            ),
            Some(3_600)
        );
        // The served model beats the bound one: a seat bound to Claude that
        // last ran GPT holds GPT's cache.
        assert_eq!(
            ttl(
                served("gpt-6.2-astra"),
                seat(Harness::Copilot, "claude-opus-5.5")
            ),
            Some(1_800)
        );
    }

    #[test]
    fn cache_state_is_unknown_without_a_last_call_or_a_ttl() {
        let no_ttl = SeatStatus {
            last_call_at_ms: Fact::native(1),
            ..SeatStatus::unknown()
        };
        assert_eq!(cache_state(&no_ttl, 2), Fact::Unknown);
        let no_call = SeatStatus {
            cache_ttl: Fact::derived(CacheTtl::ONE_HOUR),
            ..SeatStatus::unknown()
        };
        assert_eq!(cache_state(&no_call, 2), Fact::Unknown);
    }

    #[test]
    fn a_future_last_call_counts_as_just_made() {
        let status = warm_capable(10_000, CacheTtl::FIVE_MINUTES);
        assert_eq!(
            cache_state(&status, 5_000),
            Fact::derived(CacheState::Warm {
                expires_in_ms: 305_000
            })
        );
    }

    #[test]
    fn facts_round_trip_and_unknown_never_carries_a_value() {
        let facts: Vec<Fact<u64>> = vec![
            Fact::native(0),
            Fact::derived(7),
            Fact::Known {
                value: 200_000,
                basis: Basis::Table { version: 3 },
            },
            Fact::Known {
                value: 9,
                basis: Basis::MtimeFallback,
            },
            Fact::Unknown,
        ];
        let wire = serde_json::to_value(&facts).expect("serialize");
        assert_eq!(
            wire,
            serde_json::json!([
                {"value": 0, "basis": "native"},
                {"value": 7, "basis": "derived"},
                {"value": 200_000, "basis": "table@v3"},
                {"value": 9, "basis": "mtime-fallback"},
                {"basis": "unknown"},
            ])
        );
        let back: Vec<Fact<u64>> = serde_json::from_value(wire).expect("deserialize");
        assert_eq!(back, facts);
    }

    #[test]
    fn malformed_facts_are_refused_not_defaulted() {
        for bad in [
            serde_json::json!({"value": 0, "basis": "unknown"}),
            serde_json::json!({"basis": "native"}),
            serde_json::json!({"value": 1, "basis": "guessed"}),
            serde_json::json!({"value": 1, "basis": "table@vX"}),
            serde_json::json!({"value": 1}),
        ] {
            assert!(
                serde_json::from_value::<Fact<u64>>(bad.clone()).is_err(),
                "accepted {bad}"
            );
        }
    }
}
