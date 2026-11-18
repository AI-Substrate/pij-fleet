//! The request rewrite (blueprint 003 c-proxy-core): the only changes the proxy
//! makes to what a seat sends.
//!
//! - every ephemeral `cache_control` without a `ttl` gets `ttl: "1h"`, so an
//!   idle seat reads its context from cache instead of rebuilding it (req-01);
//! - `X-Initiator` is `agent` when the last message only returns tool results,
//!   so a tool continuation is not billed as a new user request (req-07);
//! - a `*-1m` model selector becomes its base id, because Copilot's API refuses
//!   `-1m` (400 `model_not_supported`) and serves 1M on the base id (req-13).
//!
//! Anything else, and any body that is not JSON, passes through unchanged.

use serde_json::Value;

/// The TTL written into every ephemeral `cache_control` that has none.
pub const CACHE_TTL: &str = "1h";

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

/// Rewrite a Messages request body. A body that does not parse as JSON is
/// forwarded byte-for-byte with the `user` initiator.
#[must_use]
pub fn rewrite(raw: &[u8]) -> Rewritten {
    let Ok(mut json) = serde_json::from_slice::<Value>(raw) else {
        return Rewritten {
            body: None,
            initiator: "user",
            model: String::new(),
            ttl_rewrites: 0,
        };
    };
    let initiator = initiator(&json);
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
    let ttl_rewrites = set_ttl(&mut json, CACHE_TTL);
    // Nothing changed: forward the client's own bytes. Serialising a `Value`
    // that was just parsed cannot fail.
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
        let out = rewrite(&raw);
        let body: Value = serde_json::from_slice(&out.body.unwrap()).unwrap();
        assert_eq!(body["model"], "claude-opus-5.5");
        assert_eq!(out.model, "claude-opus-5.5 (from claude-opus-5.5-1m)");

        // Nothing to change: the client's own bytes go upstream.
        let raw = serde_json::to_vec(&json!({"model": "claude-opus-5.5", "messages": []})).unwrap();
        let out = rewrite(&raw);
        assert_eq!(out.body, None);
        assert_eq!(out.model, "claude-opus-5.5");
        assert_eq!(base_model("-1m"), None);
    }

    #[test]
    fn a_body_that_is_not_json_is_forwarded_untouched() {
        let out = rewrite(b"not json {");
        assert_eq!(out.body, None);
        assert_eq!(out.initiator, "user");
        assert_eq!(out.ttl_rewrites, 0);
    }
}
