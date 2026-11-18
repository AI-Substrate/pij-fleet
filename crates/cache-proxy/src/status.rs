//! The status route's body (blueprint 003 c-status-route). This shape is the
//! contract the daemon's spawn gate, watch and doctor row (s03) read; change it
//! only with pm-004.
//!
//! `GET /__pij/status` returns `{ready, state, reason, fix, account,
//! upstream_host, quota: {usable, reset}, models: {<base id>:
//! {max_prompt_tokens}}, counters: {requests, refreshes, gh_launches,
//! upstream_401, upstream_402}, pid, since}`. It never carries a token or a
//! header value.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where the sidecar stands. Only [`State::Ready`] lets a spawn be proxied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Token held, endpoint known, quota usable, switched on.
    Ready,
    /// `pij-rs cache-proxy off`.
    Off,
    /// No config file, or no account in it.
    NoAccount,
    /// `gh` was not found at start.
    GhMissing,
    /// gh gave no token for the account, the token was refused, or a refresh failed.
    NoToken,
    /// The account's premium quota is used up, or it has no Copilot entitlement.
    QuotaExhausted,
    /// `copilot_internal/user` or `/models` could not be reached.
    UpstreamUnreachable,
}

impl State {
    /// Every state, in contract order.
    pub const ALL: [State; 7] = [
        State::Ready,
        State::Off,
        State::NoAccount,
        State::GhMissing,
        State::NoToken,
        State::QuotaExhausted,
        State::UpstreamUnreachable,
    ];

    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Off => "off",
            State::NoAccount => "no_account",
            State::GhMissing => "gh_missing",
            State::NoToken => "no_token",
            State::QuotaExhausted => "quota_exhausted",
            State::UpstreamUnreachable => "upstream_unreachable",
        }
    }
}

/// The account's premium-request quota.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Quota {
    /// `unlimited`, `remaining > 0` or `overage_permitted`; false when unknown.
    pub usable: bool,
    /// `quota_reset_date`, when the account reports one.
    pub reset: Option<String>,
}

/// One Claude model the account's `/models` lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModelLimit {
    /// `capabilities.limits.max_prompt_tokens`.
    pub max_prompt_tokens: u64,
}

/// Running totals since the sidecar started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Counters {
    /// Requests forwarded upstream (not counting retries or status reads).
    pub requests: u64,
    /// Token refreshes attempted after a 401 (each one `gh` launch).
    pub refreshes: u64,
    /// Every `gh auth token` launch: start, account change and refreshes.
    pub gh_launches: u64,
    /// 401s from upstream.
    pub upstream_401: u64,
    /// 402s from upstream.
    pub upstream_402: u64,
}

/// The status route's body.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Status {
    /// True only in state `ready`.
    pub ready: bool,
    /// See [`State`].
    pub state: State,
    /// Why it is not ready; empty when ready.
    pub reason: String,
    /// The command or step that fixes it; empty when ready.
    pub fix: String,
    /// The configured gh login.
    pub account: Option<String>,
    /// The upstream host from `endpoints.api`, host only.
    pub upstream_host: Option<String>,
    /// See [`Quota`].
    pub quota: Quota,
    /// Claude models by base id (never `-1m`).
    pub models: BTreeMap<String, ModelLimit>,
    /// See [`Counters`].
    pub counters: Counters,
    /// The sidecar's process id.
    pub pid: u32,
    /// Epoch milliseconds when `state` last changed.
    pub since: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// The contract s03 builds against: exact keys, nested keys and state names.
    #[test]
    fn the_status_body_has_exactly_the_contract_keys_and_state_names() {
        let mut models = BTreeMap::new();
        models.insert(
            "claude-opus-5.5".to_string(),
            ModelLimit {
                max_prompt_tokens: 1_000_000,
            },
        );
        let status = Status {
            ready: true,
            state: State::Ready,
            reason: String::new(),
            fix: String::new(),
            account: Some("octo_corp".into()),
            upstream_host: Some("api.enterprise.githubcopilot.com".into()),
            quota: Quota {
                usable: true,
                reset: None,
            },
            models,
            counters: Counters::default(),
            pid: 42,
            since: 1_763_430_000_000,
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(
            keys(&value),
            [
                "account",
                "counters",
                "fix",
                "models",
                "pid",
                "quota",
                "ready",
                "reason",
                "since",
                "state",
                "upstream_host"
            ]
        );
        assert_eq!(keys(&value["quota"]), ["reset", "usable"]);
        assert_eq!(
            keys(&value["counters"]),
            [
                "gh_launches",
                "refreshes",
                "requests",
                "upstream_401",
                "upstream_402"
            ]
        );
        assert_eq!(
            keys(&value["models"]["claude-opus-5.5"]),
            ["max_prompt_tokens"]
        );
        assert!(value["since"].is_u64() && value["pid"].is_u64());
        assert_eq!(value["quota"]["reset"], Value::Null);

        let names: Vec<Value> = State::ALL
            .iter()
            .map(|state| serde_json::to_value(state).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "ready",
                "off",
                "no_account",
                "gh_missing",
                "no_token",
                "quota_exhausted",
                "upstream_unreachable"
            ]
        );
        for state in State::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
    }
}
