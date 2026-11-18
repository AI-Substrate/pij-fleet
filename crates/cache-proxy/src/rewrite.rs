//! The request rewrite (blueprint 003 c-proxy-core): the only changes the proxy
//! makes to what a seat sends.
//!
//! - every ephemeral `cache_control` without a `ttl` gets `ttl: "1h"` on a
//!   main-agent request, so an idle seat reads its context from cache instead of
//!   rebuilding it (req-01); a subagent request (`x-interaction-type:
//!   conversation-subagent`) stays on the 5-minute default, because a short
//!   burst would pay the 2x 1-hour write for nothing (req-14);
//! - `X-Initiator` is the client's own `agent`/`user` when it sends one, else
//!   `agent` when the last message only returns tool results, so a tool
//!   continuation is not billed as a new user request (req-07, req-14);
//! - a `*-1m` model selector becomes its base id, because Copilot's API refuses
//!   `-1m` (400 `model_not_supported`) and serves 1M on the base id (req-13).
//!
//! Anything else, and any body that is not JSON, passes through unchanged.

use serde_json::Value;

/// The TTL written into every ephemeral `cache_control` that has none.
pub const CACHE_TTL: &str = "1h";

/// The `x-interaction-type` value Copilot sends on a subagent's requests.
pub const SUBAGENT_INTERACTION: &[u8] = b"conversation-subagent";

/// Whose request it is. Logged as this enum only, never the header's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    /// The seat's own agent: gets the 1-hour cache.
    Main,
    /// A subagent: keeps the 5-minute default.
    Subagent,
}

impl Agent {
    /// From the request's `x-interaction-type` header, if any.
    #[must_use]
    pub fn from_interaction_type(value: Option<&[u8]>) -> Self {
        if value == Some(SUBAGENT_INTERACTION) {
            Agent::Subagent
        } else {
            Agent::Main
        }
    }

    /// The log word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Main => "main",
            Agent::Subagent => "subagent",
        }
    }
}

/// The client's own `X-Initiator`, when it is one of the two values Copilot
/// bills by; anything else is ignored and the proxy infers it.
#[must_use]
pub fn client_initiator(value: Option<&[u8]>) -> Option<&'static str> {
    match value {
        Some(b"agent") => Some("agent"),
        Some(b"user") => Some("user"),
        _ => None,
    }
}

/// What [`rewrite`] did to one `/v1/messages` body.
#[derive(Debug, PartialEq, Eq)]
pub struct Rewritten {
    /// The body to forward: `None` means the original bytes, untouched.
    pub body: Option<Vec<u8>>,
    /// The `X-Initiator` header value.
    pub initiator: &'static str,
    /// The model for the log line: `<base> (from <x>-1m)` when it was mapped.
    pub model: String,
    /// How many `cache_control` blocks gained a TTL.
    pub ttl_rewrites: usize,
}

/// Rewrite a Messages request body for `agent`, keeping `client` (from
/// [`client_initiator`]) as the initiator when the client sent one. A body that
/// does not parse as JSON is forwarded byte-for-byte.
#[must_use]
pub fn rewrite(raw: &[u8], client: Option<&'static str>, agent: Agent) -> Rewritten {
    let Ok(mut json) = serde_json::from_slice::<Value>(raw) else {
        return Rewritten {
            body: None,
            initiator: client.unwrap_or("user"),
            model: String::new(),
            ttl_rewrites: 0,
        };
    };
    let initiator = client.unwrap_or_else(|| initiator(&json));
    let mut model = json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mapped = base_model(&model).map(str::to_string);
    if let Some(base) = &mapped {
        json["model"] = Value::String(base.clone());
        model = format!("{base} (from {model})");
    }
    let ttl_rewrites = match agent {
        Agent::Main => set_ttl(&mut json, CACHE_TTL),
        Agent::Subagent => 0,
    };
    // Nothing changed: forward the client's own bytes. Serialising a `Value`
    // that was just parsed cannot fail. Note (#239 N001): a changed body is
    // re-serialised with object keys sorted (serde_json without `preserve_order`,
    // which stays off workspace-wide): JSON-equivalent, deterministic, not byte-equal.
    let body = if mapped.is_some() || ttl_rewrites > 0 {
        serde_json::to_vec(&json).ok()
    } else {
        None
    };
    Rewritten {
        body,
        initiator,
        model,
        ttl_rewrites,
    }
}

/// The base id of a `-1m` selector (`claude-opus-5.5-1m` → `claude-opus-5.5`),
/// or `None` when the model has no such suffix.
#[must_use]
pub fn base_model(model: &str) -> Option<&str> {
    model.strip_suffix("-1m").filter(|base| !base.is_empty())
}

/// Add `ttl` to every ephemeral `cache_control` lacking one, at any depth.
/// An explicit TTL is the client's choice and is kept. Returns the count.
pub fn set_ttl(value: &mut Value, ttl: &str) -> usize {
    match value {
        Value::Object(map) => {
            let mut changed = 0;
            if let Some(Value::Object(control)) = map.get_mut("cache_control")
                && control.get("type").and_then(Value::as_str) == Some("ephemeral")
                && !control.contains_key("ttl")
            {
                control.insert("ttl".into(), Value::String(ttl.into()));
                changed += 1;
            }
            for (key, child) in map.iter_mut() {
                if key != "cache_control" {
                    changed += set_ttl(child, ttl);
                }
            }
            changed
        }
        Value::Array(items) => items.iter_mut().map(|item| set_ttl(item, ttl)).sum(),
        _ => 0,
    }
}

/// `agent` when the last message is a user message made only of
/// `tool_result` blocks (a continuation), else `user`.
#[must_use]
pub fn initiator(body: &Value) -> &'static str {
    let last = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last());
    let only_tool_results = last
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content").and_then(Value::as_array))
        .is_some_and(|blocks| {
            !blocks.is_empty()
                && blocks
                    .iter()
                    .all(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        });
    if only_tool_results { "agent" } else { "user" }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_ttl_is_added_at_every_depth_and_an_explicit_one_is_kept() {
        let mut body = json!({
            "system": [{"type": "text", "text": "s", "cache_control": {"type": "ephemeral"}}],
            "tools": [{"name": "t", "cache_control": {"type": "ephemeral"}}],
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "a", "cache_control": {"type": "ephemeral", "ttl": "5m"}},
                {"type": "text", "text": "b", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "c", "cache_control": {"type": "other"}}
            ]}]
        });
        assert_eq!(set_ttl(&mut body, "1h"), 3);
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(body["tools"][0]["cache_control"]["ttl"], "1h");
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["cache_control"]["ttl"], "5m");
        assert_eq!(content[1]["cache_control"]["ttl"], "1h");
        assert!(content[2]["cache_control"].get("ttl").is_none());
    }

    #[test]
    fn only_a_last_user_message_of_tool_results_is_an_agent_continuation() {
        let tool_results = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "go"}]},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "1"}]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "1"},
                {"type": "tool_result", "tool_use_id": "2"}
            ]}
        ]});
        assert_eq!(initiator(&tool_results), "agent");
        let mixed = json!({"messages": [{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "1"},
            {"type": "text", "text": "and also"}
        ]}]});
        assert_eq!(initiator(&mixed), "user");
        let plain_string = json!({"messages": [{"role": "user", "content": "hello"}]});
        assert_eq!(initiator(&plain_string), "user");
        let empty = json!({"messages": [{"role": "user", "content": []}]});
        assert_eq!(initiator(&empty), "user");
        let assistant_last = json!({"messages": [{"role": "assistant", "content": [
            {"type": "tool_result", "tool_use_id": "1"}
        ]}]});
        assert_eq!(initiator(&assistant_last), "user");
        assert_eq!(initiator(&json!({})), "user");
    }

    #[test]
    fn a_1m_selector_is_forwarded_as_its_base_id() {
        let raw =
            serde_json::to_vec(&json!({"model": "claude-opus-5.5-1m", "messages": []})).unwrap();
        let out = rewrite(&raw, None, Agent::Main);
        let body: Value = serde_json::from_slice(&out.body.unwrap()).unwrap();
        assert_eq!(body["model"], "claude-opus-5.5");
        assert_eq!(out.model, "claude-opus-5.5 (from claude-opus-5.5-1m)");

        // Nothing to change: the client's own bytes go upstream.
        let raw = serde_json::to_vec(&json!({"model": "claude-opus-5.5", "messages": []})).unwrap();
        let out = rewrite(&raw, None, Agent::Main);
        assert_eq!(out.body, None);
        assert_eq!(out.model, "claude-opus-5.5");
        assert_eq!(base_model("-1m"), None);
    }

    #[test]
    fn a_body_that_is_not_json_is_forwarded_untouched() {
        let out = rewrite(b"not json {", None, Agent::Main);
        assert_eq!(out.body, None);
        assert_eq!(out.initiator, "user");
        assert_eq!(out.ttl_rewrites, 0);
        assert_eq!(
            rewrite(b"not json {", Some("agent"), Agent::Main).initiator,
            "agent"
        );
    }

    fn cached_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "model": "claude-opus-5.5",
            "system": [{"type": "text", "text": "s", "cache_control": {"type": "ephemeral"}}],
            "messages": [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "1"}]}]
        }))
        .unwrap()
    }

    /// req-14 rule 1: only main-agent requests get the 1-hour TTL.
    #[test]
    fn a_subagent_request_keeps_the_5_minute_default() {
        assert_eq!(
            Agent::from_interaction_type(Some(b"conversation-subagent")),
            Agent::Subagent
        );
        assert_eq!(
            Agent::from_interaction_type(Some(b"conversation-agent")),
            Agent::Main
        );
        assert_eq!(Agent::from_interaction_type(None), Agent::Main);

        let sub = rewrite(&cached_body(), None, Agent::Subagent);
        assert_eq!(sub.ttl_rewrites, 0);
        assert_eq!(sub.body, None, "a subagent's body goes upstream untouched");

        let main = rewrite(&cached_body(), None, Agent::Main);
        assert_eq!(main.ttl_rewrites, 1);
        let body: Value = serde_json::from_slice(&main.body.unwrap()).unwrap();
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");

        // A subagent's -1m model is still mapped.
        let raw =
            serde_json::to_vec(&json!({"model": "claude-opus-5.5-1m", "messages": []})).unwrap();
        let sub = rewrite(&raw, None, Agent::Subagent);
        let body: Value = serde_json::from_slice(&sub.body.unwrap()).unwrap();
        assert_eq!(body["model"], "claude-opus-5.5");
    }

    /// req-14 rule 2: the client's own X-Initiator wins; inference is the fallback.
    #[test]
    fn the_clients_own_initiator_wins_over_inference() {
        assert_eq!(client_initiator(Some(b"agent")), Some("agent"));
        assert_eq!(client_initiator(Some(b"user")), Some("user"));
        assert_eq!(client_initiator(Some(b"Agent")), None);
        assert_eq!(client_initiator(Some(b"robot")), None);
        assert_eq!(client_initiator(None), None);
        // The body says continuation (tool results last); the client says user.
        assert_eq!(
            rewrite(&cached_body(), Some("user"), Agent::Main).initiator,
            "user"
        );
        assert_eq!(
            rewrite(&cached_body(), None, Agent::Main).initiator,
            "agent"
        );
        let fresh =
            serde_json::to_vec(&json!({"messages": [{"role": "user", "content": "hi"}]})).unwrap();
        assert_eq!(
            rewrite(&fresh, Some("agent"), Agent::Main).initiator,
            "agent"
        );
        assert_eq!(rewrite(&fresh, None, Agent::Main).initiator, "user");
    }
}
