//! The Claude Stop hook's pij side when a reply ends with a pij block (plan
//! 226). `claude-stop-pij.sh` hands the raw Stop input here only when it holds a
//! pij fence. This one launch runs the block, publishes the seat idle unless it
//! gets a follow-up turn, and prints the hook's answer, so a stop still costs one
//! `pij-rs` launch, block or not.

use std::time::Duration;

use pij_core::model::{Envelope, ErrorKind};
use serde_json::{Value, json};

use crate::client::{CallerContext, DaemonClient};

/// The daemon route that runs a reply's closing pij block.
pub const TURN_END_ROUTE: &str = "/v1/turn-end";
/// A block of up to 20 commands runs in order; a wedged daemon must not hold
/// the stop past the hook script's own watchdog.
pub const TURN_END_TIMEOUT: Duration = Duration::from_secs(30);

/// The Stop hook input fields this needs; Claude sends more.
#[derive(Debug, Default, serde::Deserialize)]
pub struct StopInput {
    /// Claude's session, the seat's native session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The session's working directory; `output --body-file` resolves against it.
    #[serde(default)]
    pub cwd: Option<String>,
    /// True when this stop ends a turn a Stop hook's block started.
    #[serde(default)]
    pub stop_hook_active: bool,
    /// The reply's full final text.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
}

/// The cheap tail check before any daemon call: the text, trimmed at the end,
/// closes a fence, and some line opens a `pij` one. The daemon parses for real.
#[must_use]
pub fn ends_with_pij_fence(text: &str) -> bool {
    text.trim_end().ends_with("```")
        && text
            .lines()
            .any(|line| line.trim_start().starts_with("```pij"))
}

/// What the hook prints, and whether the seat is idle now.
#[derive(Debug, PartialEq, Eq)]
pub struct StopAnswer {
    /// The Stop hook's stdout JSON: one line for the human, and on failure the
    /// block that gives the seat one follow-up turn.
    pub output: Option<Value>,
    /// False only when the seat keeps working on a follow-up turn.
    pub publish_idle: bool,
}

/// Decide from the daemon's answer. `followup` set means the block failed; the
/// seat gets its one follow-up turn only when this stop is not already one.
#[must_use]
pub fn answer(response: &Envelope<Value>, stop_hook_active: bool) -> StopAnswer {
    let (summary, followup) = if response.ok {
        let field = |name: &str| {
            response
                .data
                .as_ref()
                .and_then(|data| data[name].as_str())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        };
        (field("summary"), field("followup"))
    } else {
        let (summary, followup) = unreached(response);
        (Some(summary), Some(followup))
    };
    let Some(followup) = followup else {
        return StopAnswer {
            output: summary.map(|summary| json!({"systemMessage": summary})),
            publish_idle: true,
        };
    };
    let summary = summary.unwrap_or_else(|| "pij ✗ block failed".to_string());
    if stop_hook_active {
        return StopAnswer {
            output: Some(json!({
                "systemMessage": format!("{summary} (no follow-up: this turn was already one)"),
            })),
            publish_idle: true,
        };
    }
    StopAnswer {
        output: Some(json!({"decision": "block", "reason": followup, "systemMessage": summary})),
        publish_idle: false,
    }
}

/// The daemon gave no result. A refusal it sent ran nothing; a lost or
/// timed-out call may have run some of the block, so the seat must not resend
/// blindly.
fn unreached(response: &Envelope<Value>) -> (String, String) {
    let why = response
        .meta
        .as_deref()
        .and_then(|meta| meta.lines().next())
        .map(|line| line.chars().take(160).collect::<String>())
        .filter(|line| !line.trim().is_empty())
        .unwrap_or_else(|| "the daemon did not answer".to_string());
    if response.error == Some(ErrorKind::Adapter) {
        (
            format!("pij ? block outcome unknown: {why}"),
            format!(
                "[pij] Your closing pij block's outcome is unknown ({why}). Some commands may have run. Do not resend blindly; check first."
            ),
        )
    } else {
        (
            format!("pij ✗ block not run: the daemon refused it ({why})"),
            format!(
                "[pij] Your closing pij block did not run: the daemon refused it ({why}). Nothing ran. Send what still matters with tool calls."
            ),
        )
    }
}

/// What one Stop hook call did.
#[derive(Debug)]
pub struct StopRun {
    /// The decision and the hook's stdout.
    pub answer: StopAnswer,
    /// Why publishing idle failed, for the hook log.
    pub idle_error: Option<String>,
}

/// Run a Stop hook input: the block when the reply ends with one, then idle
/// unless the seat keeps working. Binding is the pane and Claude's session,
/// never an inherited seat id (plan 157).
pub async fn run(client: &DaemonClient, pane: &str, input: &str) -> StopRun {
    let stop: StopInput = serde_json::from_str(input.trim()).unwrap_or_default();
    let text = stop.last_assistant_message.as_deref().unwrap_or_default();
    let answer = if ends_with_pij_fence(text) {
        let caller = CallerContext {
            pane: Some(pane.to_string()),
            claude_session: stop.session_id.clone(),
            cwd: stop.cwd.clone().or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|cwd| cwd.display().to_string())
            }),
            ..CallerContext::default()
        };
        let body = json!({"text": text, "caller": caller, "via": "hook:claude"});
        let response = tokio::time::timeout(
            TURN_END_TIMEOUT,
            client.post::<Value, _>("pij turn-end", TURN_END_ROUTE, &body),
        )
        .await
        .unwrap_or_else(|_| {
            Envelope::refused(
                "pij turn-end",
                ErrorKind::Adapter,
                format!("no answer in {}s", TURN_END_TIMEOUT.as_secs()),
            )
        });
        answer(&response, stop.stop_hook_active)
    } else {
        StopAnswer {
            output: None,
            publish_idle: true,
        }
    };
    let idle_error = if answer.publish_idle {
        let published = client
            .activity(&json!({
                "seat": null,
                "pane": pane,
                "native_session": stop.session_id,
                "state": "idle",
            }))
            .await;
        (!published.ok).then(|| published.meta.unwrap_or_else(|| "refused".to_string()))
    } else {
        None
    };
    StopRun { answer, idle_error }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(data: Value) -> Envelope<Value> {
        Envelope::ok("pij turn-end", data)
    }

    #[test]
    fn the_tail_check_wants_a_closing_fence_and_a_pij_opening() {
        assert!(ends_with_pij_fence(
            "Done.\n\n```pij\nsend a \"b\"\n```\n  "
        ));
        assert!(!ends_with_pij_fence("```pij\nsend a b\n```\nAnd prose."));
        assert!(!ends_with_pij_fence("```bash\nls\n```"));
        assert!(!ends_with_pij_fence(""));
    }

    #[test]
    fn a_block_that_ran_shows_one_line_and_the_seat_goes_idle() {
        let answer = answer(
            &ok(json!({"outcome": "ran", "ran": 2, "summary": "pij ✓ 2 sent", "followup": null})),
            false,
        );
        assert_eq!(
            answer,
            StopAnswer {
                output: Some(json!({"systemMessage": "pij ✓ 2 sent"})),
                publish_idle: true,
            }
        );
        let none = super::answer(&ok(json!({"outcome": "none", "summary": null})), false);
        assert_eq!(
            none,
            StopAnswer {
                output: None,
                publish_idle: true
            }
        );
    }

    #[test]
    fn a_failed_block_gets_one_follow_up_turn_and_stays_working() {
        let refused = ok(json!({
            "outcome": "refused",
            "ran": 1,
            "summary": "pij ✗ line 2 refused (1 ran)",
            "followup": "Ran, do not resend: line 1.",
        }));
        assert_eq!(
            answer(&refused, false),
            StopAnswer {
                output: Some(json!({
                    "decision": "block",
                    "reason": "Ran, do not resend: line 1.",
                    "systemMessage": "pij ✗ line 2 refused (1 ran)",
                })),
                publish_idle: false,
            }
        );
        assert_eq!(
            answer(&refused, true),
            StopAnswer {
                output: Some(json!({
                    "systemMessage": "pij ✗ line 2 refused (1 ran) (no follow-up: this turn was already one)",
                })),
                publish_idle: true,
            },
            "a follow-up's own stop never blocks"
        );
    }

    #[test]
    fn a_refusal_ran_nothing_and_a_lost_call_may_have_run_some() {
        let refused =
            Envelope::<Value>::refused("pij turn-end", ErrorKind::Refused, "bad body\nsecond line");
        let output = answer(&refused, false).output.expect("output");
        assert_eq!(
            output["systemMessage"],
            "pij ✗ block not run: the daemon refused it (bad body)"
        );
        assert!(
            output["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Nothing ran"))
        );

        let lost = Envelope::<Value>::refused(
            "pij turn-end",
            ErrorKind::Adapter,
            "could not reach the daemon: connection reset",
        );
        let output = answer(&lost, false).output.expect("output");
        assert_eq!(
            output["systemMessage"],
            "pij ? block outcome unknown: could not reach the daemon: connection reset"
        );
        assert!(
            output["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Do not resend blindly"))
        );
    }
}
