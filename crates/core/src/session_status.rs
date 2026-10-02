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
pub const SEAT_STATUS_VERSION: u32 = 1;

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
}

impl Basis {
    fn to_wire(self) -> String {
        match self {
            Self::Native => "native".to_string(),
            Self::Derived => "derived".to_string(),
            Self::Table { version } => format!("table@v{version}"),
            Self::MtimeFallback => "mtime-fallback".to_string(),
        }
    }

    fn from_wire(text: &str) -> Option<Self> {
        match text {
            "native" => Some(Self::Native),
            "derived" => Some(Self::Derived),
            "mtime-fallback" => Some(Self::MtimeFallback),
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

/// How long the provider keeps the last call's prompt cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheTtl {
    /// Anthropic's default ephemeral cache.
    FiveMinutes,
    /// Anthropic's extended cache.
    OneHour,
}

impl CacheTtl {
    /// The TTL in milliseconds.
    pub const fn as_ms(self) -> u64 {
        match self {
            Self::FiveMinutes => 5 * 60 * 1_000,
            Self::OneHour => 60 * 60 * 1_000,
        }
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
    /// The cache TTL the last call wrote, inferred from its write split.
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
/// cold seat warm, which is the expensive error. A last call stamped after
/// `now_ms` (clock skew) counts as just made.
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
        let status = warm_capable(1_000, CacheTtl::FiveMinutes);
        assert_eq!(
            cache_state(&status, 1_000 + 299_999),
            Fact::derived(CacheState::Warm { expires_in_ms: 1 })
        );
        assert_eq!(
            cache_state(&status, 1_000 + 300_000),
            Fact::derived(CacheState::Cold { expired_for_ms: 0 })
        );
        let hour = warm_capable(0, CacheTtl::OneHour);
        assert_eq!(
            cache_state(&hour, 600_000),
            Fact::derived(CacheState::Warm {
                expires_in_ms: 3_000_000
            })
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
            cache_ttl: Fact::derived(CacheTtl::OneHour),
            ..SeatStatus::unknown()
        };
        assert_eq!(cache_state(&no_call, 2), Fact::Unknown);
    }

    #[test]
    fn a_future_last_call_counts_as_just_made() {
        let status = warm_capable(10_000, CacheTtl::FiveMinutes);
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
