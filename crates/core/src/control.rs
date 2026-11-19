//! Remote control command policy shared by the daemon and its clients.

use crate::model::{Harness, SeatDescriptor, SeatId};
use serde::{Deserialize, Serialize};

/// The remote controls every harness can take.
pub const ALLOWED_COMMANDS: [&str; 3] = ["compact", "new", "reload"];
/// The controls pij types into a Copilot pane (plan 186). Copilot messages stay
/// on its native extension; only these ride the pane control path. `restart`
/// is Copilot's own `/restart`, which swaps the CLI process and restores the
/// same live session (F212); no other harness takes it.
pub const COPILOT_COMMANDS: [&str; 2] = ["compact", "restart"];
/// Every other control is left to Copilot's native, user-operated controls.
pub const COPILOT_CONTROL_REFUSAL: &str =
    "E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls";
/// A Copilot-only control sent to another harness.
pub const COPILOT_ONLY_REFUSAL: &str = "E-RS-CONTROL-UNSUPPORTED: `restart` is Copilot's /restart; other harnesses have no such control";

fn known(command: &str) -> bool {
    ALLOWED_COMMANDS.contains(&command) || COPILOT_COMMANDS.contains(&command)
}

/// What the recipient runtime observed after handling a control command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum ControlOutcome {
    /// The runtime executed the requested control.
    Executed,
    /// The runtime declined the requested control.
    Refused {
        /// Why execution was refused.
        reason: String,
    },
}

/// Validate the command name and exclusive, empty-body contract.
///
/// # Errors
/// Returns a decodable refusal code for an unknown name or non-empty body.
pub fn validate_command(command: &str, body: &str) -> Result<(), String> {
    if !known(command) {
        return Err(format!(
            "E-RS-CONTROL-INVALID: unknown command `{command}`; allowed: {} (Copilot also: restart)",
            ALLOWED_COMMANDS.join(", ")
        ));
    }
    let payload = crate::framing::message_payload(body).unwrap_or(body);
    if !payload.trim().is_empty() {
        return Err(
            "E-RS-CONTROL-BODY: --command and a message body are mutually exclusive".to_string(),
        );
    }
    Ok(())
}

/// Refuse targets that cannot execute `command`.
///
/// # Errors
/// Copilot controls other than [`COPILOT_COMMANDS`] and paneless targets return
/// stable refusal codes.
pub fn validate_target(command: &str, target: &SeatDescriptor) -> Result<(), String> {
    if target.harness == Harness::Copilot && !COPILOT_COMMANDS.contains(&command) {
        return Err(COPILOT_CONTROL_REFUSAL.to_string());
    }
    if target.harness != Harness::Copilot && !ALLOWED_COMMANDS.contains(&command) {
        return Err(COPILOT_ONLY_REFUSAL.to_string());
    }
    if target
        .pane
        .as_deref()
        .is_none_or(|pane| pane.trim().is_empty())
    {
        return Err(
            "E-RS-CONTROL-PANELESS: paneless seats cannot execute remote controls".to_string(),
        );
    }
    Ok(())
}

/// Authorize a validated command using a daemon-derived actor.
///
/// Recorded parent is the rs representation of the seat's governing prime;
/// role labels and fleet-wide prime designations confer no authority here.
///
/// # Errors
/// Context-destructive controls require self or the target's recorded parent.
pub fn authorize_command(
    command: &str,
    actor: &SeatId,
    target: &SeatDescriptor,
) -> Result<(), String> {
    if command != "compact" && actor != &target.id && target.parent.as_ref() != Some(actor) {
        return Err(format!(
            "E-RS-CONTROL-OWNERSHIP: `{command}` requires the target itself or its recorded parent"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_fixture_matches_the_authoritative_policy() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/control-commands.json"))
                .expect("fixture");
        assert_eq!(fixture["commands"], serde_json::json!(ALLOWED_COMMANDS));
        assert_eq!(fixture["copilot_refusal"], COPILOT_CONTROL_REFUSAL);
        assert_eq!(
            fixture["copilot_commands"],
            serde_json::json!(COPILOT_COMMANDS)
        );
    }

    #[test]
    fn copilot_allows_compact_and_restart_only_and_paneless_copilot_refuses_by_name() {
        let mut copilot = SeatDescriptor::new("copilot", Harness::Copilot, "/tree");
        copilot.pane = Some("%1".into());
        for command in ["compact", "restart"] {
            assert_eq!(validate_command(command, ""), Ok(()));
            assert_eq!(validate_target(command, &copilot), Ok(()));
        }
        for command in ["new", "reload"] {
            assert_eq!(
                validate_target(command, &copilot),
                Err(COPILOT_CONTROL_REFUSAL.to_string())
            );
        }
        copilot.pane = None;
        let refusal = validate_target("compact", &copilot).expect_err("paneless");
        assert!(refusal.starts_with("E-RS-CONTROL-PANELESS"), "{refusal}");
    }

    /// F212: `restart` is Copilot's `/restart`. Typed into an OMP, Claude or
    /// Codex pane it would be a prompt, so every other harness refuses it.
    #[test]
    fn restart_is_refused_for_every_other_harness() {
        for harness in [Harness::Omp, Harness::Claude, Harness::Codex, Harness::Pi] {
            let mut seat = SeatDescriptor::new("seat", harness, "/tree");
            seat.pane = Some("%1".into());
            assert_eq!(
                validate_target("restart", &seat),
                Err(COPILOT_ONLY_REFUSAL.to_string()),
                "{harness:?}"
            );
        }
    }

    #[test]
    fn control_body_allows_only_empty_envelopes() {
        for body in [
            "",
            " \n",
            "[pij from sender] ",
            "[pij-rs from sender]\n\n[/pij]",
        ] {
            assert!(validate_command("compact", body).is_ok(), "{body:?}");
        }
        for body in [
            "hello",
            "[pij from sender] hello",
            "[pij-rs from sender]\nhello\n[/pij]",
            "[pij-rs from sender]\n\n[/pij]extra",
        ] {
            assert!(validate_command("compact", body).is_err(), "{body:?}");
        }
        for command in ["", "quit", "/compact", "compact\nnew"] {
            assert!(validate_command(command, "").is_err());
        }
    }

    #[test]
    fn control_ownership_never_infers_authority_from_role() {
        let mut target = SeatDescriptor::new("target", Harness::Omp, "/tree");
        target.parent = Some("parent".into());
        target.role = Some("prime".into());
        for command in ALLOWED_COMMANDS {
            assert!(authorize_command(command, &target.id, &target).is_ok());
            assert!(authorize_command(command, &"parent".into(), &target).is_ok());
            assert_eq!(
                authorize_command(command, &"stranger".into(), &target).is_ok(),
                command == "compact"
            );
        }
    }

    #[test]
    fn control_outcomes_use_the_runtime_wire_contract() {
        for (outcome, wire) in [
            (
                ControlOutcome::Executed,
                serde_json::json!({"outcome":"executed"}),
            ),
            (
                ControlOutcome::Refused {
                    reason: "busy".to_string(),
                },
                serde_json::json!({"outcome":"refused","reason":"busy"}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&outcome).expect("encode"), wire);
            assert_eq!(
                serde_json::from_value::<ControlOutcome>(wire).expect("decode"),
                outcome
            );
        }
    }
}
