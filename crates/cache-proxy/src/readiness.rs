//! Readiness (blueprint 003 c-readiness): with the account's token, ask GitHub
//! where the account's Copilot API is, whether its premium quota is usable, and
//! which Claude models it serves with what prompt limit. Two GETs, no model
//! call and no launch (blueprint d-4).

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use crate::status::{ModelLimit, Quota, State};

/// GitHub's API, where `copilot_internal/user` lives.
pub const GITHUB_API: &str = "https://api.github.com";

/// The client identity sent upstream, as the POC sent it.
pub const COPILOT_VERSION: &str = "copilot/1.0.82";

/// How long one answer stands for an account before the next status read
/// re-runs it (blueprint k-3).
pub const CHECK_TTL: Duration = Duration::from_secs(600);

/// How long one readiness GET may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Why the account is not ready, in the status route's terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The state it puts the sidecar in.
    pub state: State,
    /// One line for a human.
    pub reason: String,
}

/// What one check found. `failure` is `None` when the account is ready.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `endpoints.api`, a full base URL.
    pub api: Option<String>,
    /// The premium quota.
    pub quota: Quota,
    /// Claude models by id.
    pub models: BTreeMap<String, ModelLimit>,
    /// Why it isn't ready.
    pub failure: Option<Failure>,
}

fn failure(state: State, reason: String) -> Option<Failure> {
    Some(Failure { state, reason })
}

/// Read `copilot_internal/user`: the endpoint and the quota.
///
/// Usable if `unlimited`, `remaining > 0` or `overage_permitted` (spike
/// REPORT.md); otherwise `quota_exhausted` with the reset date. No
/// `endpoints.api` means the account has no Copilot entitlement.
#[must_use]
pub fn parse_user(user: &Value, account: &str, allow_http: bool) -> Facts {
    let snapshot = &user["quota_snapshots"]["premium_interactions"];
    let usable = snapshot["unlimited"].as_bool() == Some(true)
        || snapshot["remaining"]
            .as_f64()
            .is_some_and(|left| left > 0.0)
        || snapshot["overage_permitted"].as_bool() == Some(true);
    let reset = user["quota_reset_date"].as_str().map(str::to_string);
    let quota = Quota {
        usable,
        reset: reset.clone(),
    };
    let api = user["endpoints"]["api"].as_str().map(str::to_string);
    let Some(api) = api else {
        return Facts {
            quota,
            failure: failure(
                State::QuotaExhausted,
                format!(
                    "no Copilot entitlement for {account}: copilot_internal/user names no endpoints.api"
                ),
            ),
            ..Facts::default()
        };
    };
    let scheme_ok = api.starts_with("https://") || (allow_http && api.starts_with("http://"));
    if !scheme_ok || host_of(&api).is_none() {
        return Facts {
            quota,
            failure: failure(
                State::UpstreamUnreachable,
                format!(
                    "copilot_internal/user named an endpoint that is not an https URL: {api:?}"
                ),
            ),
            ..Facts::default()
        };
    }
    let failure = if usable {
        None
    } else {
        failure(
            State::QuotaExhausted,
            format!(
                "premium quota exhausted for {account} until {}",
                reset.as_deref().unwrap_or("an unknown date")
            ),
        )
    };
    Facts {
        api: Some(api),
        quota,
        models: BTreeMap::new(),
        failure,
    }
}

/// Read `/models`: every id starting `claude` (never a `-1m` id) with its
/// `capabilities.limits.max_prompt_tokens`.
#[must_use]
pub fn parse_models(models: &Value) -> BTreeMap<String, ModelLimit> {
    models["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model["id"].as_str()?;
            if !id.starts_with("claude") || id.ends_with("-1m") {
                return None;
            }
            let max_prompt_tokens =
                model["capabilities"]["limits"]["max_prompt_tokens"].as_u64()?;
            Some((id.to_string(), ModelLimit { max_prompt_tokens }))
        })
        .collect()
}

/// The host (and port, if any) of a base URL.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn get(http: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    http.get(url)
        .timeout(CHECK_TIMEOUT)
        .bearer_auth(token)
        .header("user-agent", COPILOT_VERSION)
        .header("editor-version", COPILOT_VERSION)
        .header("copilot-integration-id", "copilot-developer-cli")
}

/// Fetch one JSON document, mapping a refusal to the state it means.
async fn fetch(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    account: &str,
    what: &str,
) -> Result<Value, Failure> {
    let unreachable = |reason: String| Failure {
        state: State::UpstreamUnreachable,
        reason,
    };
    let response = get(http, url, token)
        .send()
        .await
        .map_err(|error| unreachable(format!("{what} unreachable: {}", error.without_url())))?;
    match response.status() {
        status if status.is_success() => response.json().await.map_err(|error| {
            unreachable(format!(
                "{what} answered unreadable JSON: {}",
                error.without_url()
            ))
        }),
        StatusCode::UNAUTHORIZED => Err(Failure {
            state: State::NoToken,
            reason: format!("{what} refused {account}'s token (401)"),
        }),
        status @ (StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) => Err(Failure {
            state: State::QuotaExhausted,
            reason: format!("no Copilot entitlement for {account}: {what} answered {status}"),
        }),
        status => Err(unreachable(format!("{what} answered {status}"))),
    }
}

/// Run both GETs for `account` with `token`.
pub async fn check(
    http: &reqwest::Client,
    github_api: &str,
    token: &str,
    account: &str,
    allow_http: bool,
) -> Facts {
    let user_url = format!("{}/copilot_internal/user", github_api.trim_end_matches('/'));
    let user = match fetch(http, &user_url, token, account, "copilot_internal/user").await {
        Ok(user) => user,
        Err(failure) => {
            return Facts {
                failure: Some(failure),
                ..Facts::default()
            };
        }
    };
    let mut facts = parse_user(&user, account, allow_http);
    let Some(api) = facts.api.clone() else {
        return facts;
    };
    let models_url = format!("{}/models", api.trim_end_matches('/'));
    match fetch(http, &models_url, token, account, "/models").await {
        Ok(models) => facts.models = parse_models(&models),
        // A quota failure already says why; otherwise the models failure does.
        Err(failure) if facts.failure.is_none() => facts.failure = Some(failure),
        Err(_) => {}
    }
    facts
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn user(snapshot: Value) -> Value {
        json!({
            "endpoints": {"api": "https://api.enterprise.githubcopilot.com"},
            "quota_snapshots": {"premium_interactions": snapshot},
            "quota_reset_date": "2026-12-01"
        })
    }

    #[test]
    fn the_quota_is_usable_if_unlimited_or_remaining_or_overage_permitted() {
        for snapshot in [
            json!({"unlimited": true, "remaining": 0}),
            json!({"remaining": 955_986, "overage_permitted": false}),
            json!({"remaining": 0, "overage_permitted": true}),
        ] {
            let facts = parse_user(&user(snapshot.clone()), "a", false);
            assert!(facts.quota.usable, "{snapshot}");
            assert_eq!(facts.failure, None, "{snapshot}");
            assert_eq!(
                facts.api.as_deref(),
                Some("https://api.enterprise.githubcopilot.com")
            );
        }
    }

    #[test]
    fn an_exhausted_quota_names_its_reset_date() {
        let facts = parse_user(
            &user(json!({"remaining": 0, "overage_permitted": false})),
            "octocat",
            false,
        );
        assert!(!facts.quota.usable);
        assert_eq!(facts.quota.reset.as_deref(), Some("2026-12-01"));
        let failure = facts.failure.unwrap();
        assert_eq!(failure.state, State::QuotaExhausted);
        assert_eq!(
            failure.reason,
            "premium quota exhausted for octocat until 2026-12-01"
        );
        // The endpoint is still known, so status can show the host.
        assert!(facts.api.is_some());
    }

    #[test]
    fn no_endpoint_is_no_entitlement_and_a_plain_http_endpoint_is_refused() {
        let facts = parse_user(&json!({"login": "x"}), "x", false);
        assert_eq!(facts.failure.unwrap().state, State::QuotaExhausted);
        let mut plain = user(json!({"unlimited": true}));
        plain["endpoints"]["api"] = json!("http://evil.example");
        assert_eq!(
            parse_user(&plain, "x", false).failure.unwrap().state,
            State::UpstreamUnreachable
        );
        assert_eq!(parse_user(&plain, "x", true).failure, None);
    }

    #[test]
    fn models_keep_only_claude_ids_with_a_prompt_limit_and_never_1m() {
        let models = parse_models(&json!({"data": [
            {"id": "claude-opus-5.5", "capabilities": {"limits": {"max_prompt_tokens": 1_000_000}}},
            {"id": "claude-sonnet-5.5", "capabilities": {"limits": {"max_prompt_tokens": 936_000}}},
            {"id": "claude-opus-5.5-1m", "capabilities": {"limits": {"max_prompt_tokens": 1_000_000}}},
            {"id": "gpt-6.1-sol", "capabilities": {"limits": {"max_prompt_tokens": 400_000}}},
            {"id": "claude-no-limit", "capabilities": {}}
        ]}));
        assert_eq!(
            models.keys().collect::<Vec<_>>(),
            ["claude-opus-5.5", "claude-sonnet-5.5"]
        );
        assert_eq!(models["claude-opus-5.5"].max_prompt_tokens, 1_000_000);
    }

    #[test]
    fn the_host_is_the_host_only() {
        assert_eq!(
            host_of("https://api.enterprise.githubcopilot.com").as_deref(),
            Some("api.enterprise.githubcopilot.com")
        );
        assert_eq!(
            host_of("http://127.0.0.1:9/x").as_deref(),
            Some("127.0.0.1:9")
        );
        assert_eq!(host_of("nonsense"), None);
    }
}
