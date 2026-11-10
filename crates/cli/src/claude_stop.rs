//! The Claude Stop hook's pij side when a reply ends with a pij block (plan
//! 226). `claude-stop-pij.sh` hands the raw Stop input here only when it holds a
//! pij fence. This one launch runs the block, publishes the seat idle unless it
//! gets a follow-up turn, and prints the hook's answer, so a stop still costs one
//! `pij-rs` launch, block or not.

use std::time::Duration;

use pij_core::model::{Envelope, ErrorKind};
use pij_core::turn_end::{TurnEndResult, may_end_with_block};
use serde_json::{Value, json};

use crate::client::{CallerContext, DaemonClient, TurnEndRequest};

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
pub fn answer(response: &Envelope<TurnEndResult>, stop_hook_active: bool) -> StopAnswer {
    let (summary, followup) = if response.ok {
        let nonblank =
            |field: Option<&String>| field.filter(|value| !value.trim().is_empty()).cloned();
        let data = response.data.as_ref();
        (
            nonblank(data.and_then(|data| data.summary.as_ref())),
            nonblank(data.and_then(|data| data.followup.as_ref())),
        )
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
/// timed-out call may have run some of the block, so the seat must check
/// before resending. The same wording as the OMP and Copilot hooks.
fn unreached(response: &Envelope<TurnEndResult>) -> (String, String) {
    let reason = clip_reason(response.meta.as_deref().unwrap_or_default());
    let reason = if reason.is_empty() {
        "the daemon did not answer".to_string()
    } else {
        reason
    };
    if response.error == Some(ErrorKind::Adapter) {
        (
            format!("pij ? block outcome unknown: {reason}"),
            format!(
                "[pij] Your end-of-turn pij block's outcome is unknown ({reason}): some of its commands may have run.\nCheck before you resend any of them, or say why not."
            ),
        )
    } else {
        (
            format!("pij ✗ block not run: the daemon refused it ({reason})"),
            format!(
                "[pij] Your end-of-turn pij block did not run: the daemon refused it ({reason}). Nothing was sent.\nFix it and end your reply with a corrected pij block, or say why not."
            ),
        )
    }
}

/// How much of a no-result reason the human and the seat see.
pub const REASON_CHARS: usize = 120;

/// A transport or refusal reason: its first line, trimmed, control characters
/// escaped, cut at [`REASON_CHARS`] (as OMP's `clipReason`).
#[must_use]
pub fn clip_reason(reason: &str) -> String {
    let first = reason.split('\n').next().unwrap_or_default().trim();
    let escaped = first
        .chars()
        .flat_map(|ch| {
            if ch.is_control() {
                ch.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect::<Vec<_>>();
    if escaped.len() <= REASON_CHARS {
        return escaped.into_iter().collect();
    }
    let mut clipped = escaped[..REASON_CHARS].iter().collect::<String>();
    clipped.push('…');
    clipped
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
    let answer = if may_end_with_block(text) {
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
        let request = TurnEndRequest {
            text: text.to_string(),
            caller,
            via: Some("hook:claude".to_string()),
        };
        let response = tokio::time::timeout(TURN_END_TIMEOUT, client.turn_end(&request))
            .await
            .unwrap_or_else(|_| {
                Envelope::refused(
                    pij_daemon::http::TURN_END,
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

    use pij_core::turn_end::Outcome;

    fn ok(
        outcome: Outcome,
        summary: Option<&str>,
        followup: Option<&str>,
    ) -> Envelope<TurnEndResult> {
        Envelope::ok(
            pij_daemon::http::TURN_END,
            TurnEndResult {
                outcome,
                summary: summary.map(str::to_string),
                followup: followup.map(str::to_string),
                ..TurnEndResult::none()
            },
        )
    }

    #[test]
    fn a_block_that_ran_shows_one_line_and_the_seat_goes_idle() {
        let answer = answer(&ok(Outcome::Ran, Some("pij ✓ 2 sent"), None), false);
        assert_eq!(
            answer,
            StopAnswer {
                output: Some(json!({"systemMessage": "pij ✓ 2 sent"})),
                publish_idle: true,
            }
        );
        let none = super::answer(&ok(Outcome::None, None, None), false);
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
        let refused = ok(
            Outcome::Refused,
            Some("pij ✗ line 2 refused (1 ran)"),
            Some("Ran, do not resend: line 1."),
        );
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
        let refused = Envelope::<TurnEndResult>::refused(
            "pij turn-end",
            ErrorKind::Refused,
            "bad body\nsecond line",
        );
        let output = answer(&refused, false).output.expect("output");
        assert_eq!(
            output["systemMessage"],
            "pij ✗ block not run: the daemon refused it (bad body)"
        );
        assert!(
            output["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Nothing was sent."))
        );

        let lost = Envelope::<TurnEndResult>::refused(
            "pij turn-end",
            ErrorKind::Adapter,
            "could not reach the daemon: connection reset",
        );
        let output = answer(&lost, false).output.expect("output");
        assert_eq!(
            output["systemMessage"],
            "pij ? block outcome unknown: could not reach the daemon: connection reset"
        );
        assert!(output["reason"].as_str().is_some_and(|reason| {
            reason.contains("some of its commands may have run.\nCheck before you resend")
        }));
    }

    #[test]
    fn a_no_result_reason_is_one_escaped_clipped_line() {
        assert_eq!(
            clip_reason("  bad\u{1b}[31m body \nsecond"),
            "bad\\u{1b}[31m body"
        );
        let long = "x".repeat(REASON_CHARS + 5);
        assert_eq!(clip_reason(&long), format!("{}…", "x".repeat(REASON_CHARS)));
        assert_eq!(clip_reason(""), "");
    }
}
