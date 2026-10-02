use std::path::Path;

use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, SeatId};
use pij_core::ports::LaunchCommand;

const PIJ_SESSION_ID: &str = "PIJ_SESSION_ID";
const PIJ_SPAWN_ID: &str = "PIJ_SPAWN_ID";
const CLAUDE_ACCEPT_SETTINGS: &str = r#"{"crossSessionInbound":"accept"}"#;

/// Facts known by the launcher and the exact command it will execute.
///
/// `cross_session_inbound_accept` is derived from `args`; callers cannot stamp it
/// independently of the command that establishes the fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPlan {
    /// Harness being launched.
    pub harness: Harness,
    /// `env`, which applies [`PIJ_SESSION_ID`] before starting the harness.
    pub executable: String,
    /// The assigned identity, actual harness executable, and its discrete arguments.
    /// No shell joins this vector.
    pub args: Vec<String>,
    /// Exact requested model selector, when supplied.
    pub model: Option<String>,
    /// Provider only when model resolution established it.
    pub provider: Option<String>,
    /// Exact requested reasoning effort, when supplied.
    pub effort: Option<String>,
    /// True only when the Claude accept setting is present in `args`.
    pub cross_session_inbound_accept: bool,
}

/// Inputs used to build one spawn command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPlanInput {
    /// Seat id already assigned by the daemon under its spawn lock.
    pub seat_id: SeatId,
    /// Correlator minted for this exact launch and carried into registration.
    pub spawn_id: String,
    /// Harness to launch.
    pub harness: Harness,
    /// Absolute executable-path override from `--bin`; never a harness selector.
    pub executable: Option<String>,
    /// Exact model selector requested by the caller.
    pub model: Option<String>,
    /// Provider established by model resolution, not guessed from the selector.
    pub resolved_provider: Option<String>,
    /// Exact reasoning effort requested by the caller.
    pub effort: Option<String>,
    /// Whether to configure Claude to accept cross-session inbound messages.
    pub accept_inbound: bool,
    /// Harness-native conversation to resume (plan 156 revive); `None` is blank.
    pub resume: Option<String>,
}

/// Validate `--bin` without allowing it to change the selected harness.
///
/// # Errors
///
/// Refuses bare commands, relative paths, and a known different harness basename.
pub fn validate_executable_override(harness: Harness, executable: Option<&str>) -> Result<()> {
    let Some(executable) = executable else {
        return Ok(());
    };
    let path = Path::new(executable);
    if let Some(selected) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(Harness::parse)
        && selected != harness
    {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!(
                "--bin {executable} selects {selected}, not the requested harness {harness}; use --harness {selected} and omit --bin, or provide an absolute executable path for {harness}"
            ),
        });
    }
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!(
                "--bin is an absolute executable-path override, not a harness selector; use --harness {harness} without --bin, or --bin /absolute/path/to/{harness}"
            ),
        });
    }
    Ok(())
}

/// Build the exact environment launcher and harness argv for one spawn.
///
/// # Composition recipe
///
/// `use pij_harnesses::{SpawnPlanInput, build_spawn_plan};` in the authenticated
/// `POST /v1/spawn` handler. Assign the seat id under the spawn lock, then build
/// `SpawnPlanInput` from that exact id and launch correlator plus the request's
/// harness, executable/model/effort, and `accept_inbound`. Pass the returned
/// `plan.executable` and discrete `plan.args` as `Some(&LaunchCommand)` to
/// `Services.tmux.new_window`. Only after that call returns a pane, write the
/// pre-bind descriptor and copy the argv-derived accept stamp from the plan;
/// native extension delivery requires registration, not a launch-time stamp.
///
/// # Errors
///
/// Refuses `accept_inbound` for non-Claude harnesses because those runtimes do
/// not establish the capability represented by the descriptor stamp.
/// Executable overrides must be absolute paths and cannot name another harness.
pub fn build_spawn_plan(input: SpawnPlanInput) -> Result<SpawnPlan> {
    validate_executable_override(input.harness, input.executable.as_deref())?;
    if input.spawn_id.trim().is_empty() {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: "spawn id must be non-empty so the first bind can correlate its launch"
                .to_string(),
        });
    }
    if input.accept_inbound && input.harness != Harness::Claude {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!(
                "--accept-inbound applies only to claude, not {} — omit it for this harness",
                input.harness
            ),
        });
    }

    if input
        .resume
        .as_deref()
        .is_some_and(|session| session.trim().is_empty() || session.starts_with('-'))
    {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: "a resumed conversation id must be non-empty and not an option".to_string(),
        });
    }
    let harness_executable = input
        .executable
        .unwrap_or_else(|| input.harness.as_str().to_string());
    let mut args = vec![
        format!("{PIJ_SESSION_ID}={}", input.seat_id),
        format!("{PIJ_SPAWN_ID}={}", input.spawn_id),
        harness_executable,
    ];
    // Each CLI's own resume spelling, first, so a subcommand (codex) parses.
    if let Some(session) = &input.resume {
        match input.harness {
            Harness::Claude => args.extend(["--resume".to_string(), session.clone()]),
            Harness::Copilot | Harness::Omp => args.push(format!("--resume={session}")),
            Harness::Pi => args.extend(["--session".to_string(), session.clone()]),
            Harness::Codex => args.extend(["resume".to_string(), session.clone()]),
        }
    }
    match input.harness {
        Harness::Pi => {
            if let Some(model) = &input.model {
                let selector = input
                    .effort
                    .as_ref()
                    .map_or_else(|| model.clone(), |effort| format!("{model}:{effort}"));
                args.extend(["--model".to_string(), selector]);
            }
        }
        Harness::Omp => {
            args.push("--auto-approve".to_string());
            push_option(&mut args, "--model", input.model.as_deref());
            push_option(&mut args, "--thinking", input.effort.as_deref());
        }
        Harness::Claude => {
            args.push("--dangerously-skip-permissions".to_string());
            if input.accept_inbound {
                args.extend(["--settings".to_string(), CLAUDE_ACCEPT_SETTINGS.to_string()]);
            }
            push_option(&mut args, "--model", input.model.as_deref());
            push_option(&mut args, "--effort", input.effort.as_deref());
        }
        Harness::Copilot => {
            args.push("--yolo".to_string());
            push_option(&mut args, "--model", input.model.as_deref());
            if input.model.is_some() {
                args.extend(["--context".to_string(), "long_context".to_string()]);
            }
            push_option(&mut args, "--effort", input.effort.as_deref());
        }
        Harness::Codex => {
            args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            push_option(&mut args, "--model", input.model.as_deref());
            if let Some(effort) = &input.effort {
                args.extend(["-c".to_string(), format!("model_reasoning_effort={effort}")]);
            }
        }
    }

    let cross_session_inbound_accept = args
        .windows(2)
        .any(|pair| pair[0] == "--settings" && pair[1] == CLAUDE_ACCEPT_SETTINGS);

    Ok(SpawnPlan {
        harness: input.harness,
        executable: "env".to_string(),
        args,
        model: input.model,
        provider: input.resolved_provider,
        effort: input.effort,
        cross_session_inbound_accept,
    })
}

/// Wrap one exact harness argv with the shipped `pij-rs` child observer.
///
/// The harness still inherits tmux's PTY directly. The wrapper only records the
/// eventual exit code and keeps the pane alive briefly so the daemon can capture
/// the final visible lines; no shell joins or interprets the harness arguments.
pub fn observed_launch_command(
    plan: &SpawnPlan,
    wrapper_executable: String,
    status_path: &Path,
    log_path: &Path,
) -> Result<LaunchCommand> {
    if wrapper_executable.trim().is_empty() {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: "the child observer executable must be non-empty".to_string(),
        });
    }
    let status_path = status_path.to_str().ok_or_else(|| PijError::Adapter {
        adapter: "spawn".to_string(),
        message: "the child exit-status path is not valid UTF-8".to_string(),
    })?;
    let log_path = log_path.to_str().ok_or_else(|| PijError::Adapter {
        adapter: "spawn".to_string(),
        message: "the child log path is not valid UTF-8".to_string(),
    })?;
    let mut args = Vec::with_capacity(plan.args.len() + 7);
    args.extend([
        "__spawn-child".to_string(),
        "--status-file".to_string(),
        status_path.to_string(),
        "--log-file".to_string(),
        log_path.to_string(),
        "--".to_string(),
        plan.executable.clone(),
    ]);
    args.extend(plan.args.iter().cloned());
    Ok(LaunchCommand {
        executable: wrapper_executable,
        args,
    })
}

fn push_option(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.extend([flag.to_string(), value.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLAUDE_ACCEPT_SETTINGS, PIJ_SESSION_ID, PIJ_SPAWN_ID, SpawnPlanInput, build_spawn_plan,
    };
    use pij_core::model::{Harness, SeatId};

    fn input(harness: Harness) -> SpawnPlanInput {
        SpawnPlanInput {
            seat_id: SeatId::from("pij-assigned"),
            spawn_id: "spawn-assigned".to_string(),
            harness,
            executable: None,
            model: Some("provider/model".to_string()),
            resolved_provider: Some("provider".to_string()),
            effort: Some("high".to_string()),
            accept_inbound: false,
            resume: None,
        }
    }

    /// Plan 156 AC3: revive relaunches the recorded conversation with each
    /// harness's own resume spelling, placed where that CLI parses it.
    #[test]
    fn resume_uses_each_harness_spelling() {
        for (harness, expected) in [
            (Harness::Claude, vec!["claude", "--resume", "S-1"]),
            (Harness::Copilot, vec!["copilot", "--resume=S-1"]),
            (Harness::Omp, vec!["omp", "--resume=S-1"]),
            (Harness::Pi, vec!["pi", "--session", "S-1"]),
            (Harness::Codex, vec!["codex", "resume", "S-1"]),
        ] {
            let mut input = input(harness);
            input.resume = Some("S-1".to_string());
            let plan = build_spawn_plan(input).expect("resume plan");
            assert_eq!(
                &plan.args[2..2 + expected.len()],
                expected.as_slice(),
                "{harness}"
            );
        }
    }

    #[test]
    fn every_harness_builds_discrete_ruled_argv() {
        let cases = [
            (Harness::Pi, "pi", vec!["--model", "provider/model:high"]),
            (
                Harness::Omp,
                "omp",
                vec![
                    "--auto-approve",
                    "--model",
                    "provider/model",
                    "--thinking",
                    "high",
                ],
            ),
            (
                Harness::Claude,
                "claude",
                vec![
                    "--dangerously-skip-permissions",
                    "--model",
                    "provider/model",
                    "--effort",
                    "high",
                ],
            ),
            (
                Harness::Copilot,
                "copilot",
                vec![
                    "--yolo",
                    "--model",
                    "provider/model",
                    "--context",
                    "long_context",
                    "--effort",
                    "high",
                ],
            ),
            (
                Harness::Codex,
                "codex",
                vec![
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--model",
                    "provider/model",
                    "-c",
                    "model_reasoning_effort=high",
                ],
            ),
        ];

        for (harness, harness_executable, expected_harness_args) in cases {
            let plan = build_spawn_plan(input(harness)).expect("build plan");
            assert_eq!(plan.executable, "env");
            assert_eq!(
                plan.args,
                [
                    vec![
                        format!("{PIJ_SESSION_ID}=pij-assigned"),
                        format!("{PIJ_SPAWN_ID}=spawn-assigned"),
                        harness_executable.to_string()
                    ],
                    expected_harness_args
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ]
                .concat()
            );
            assert!(!plan.cross_session_inbound_accept);
            assert_eq!(plan.model.as_deref(), Some("provider/model"));
            assert_eq!(plan.provider.as_deref(), Some("provider"));
            assert_eq!(plan.effort.as_deref(), Some("high"));
        }
    }

    #[test]
    fn accept_stamp_is_true_only_when_exact_setting_is_emitted() {
        let without = build_spawn_plan(input(Harness::Claude)).expect("closed plan");
        assert!(!without.cross_session_inbound_accept);
        assert!(!without.args.iter().any(|arg| arg == "--settings"));
        assert_eq!(without.args[0], "PIJ_SESSION_ID=pij-assigned");

        let mut accepted_input = input(Harness::Claude);
        accepted_input.accept_inbound = true;
        let accepted = build_spawn_plan(accepted_input).expect("accepted plan");
        assert!(accepted.cross_session_inbound_accept);
        assert!(
            accepted
                .args
                .windows(2)
                .any(|pair| { pair == ["--settings", CLAUDE_ACCEPT_SETTINGS] })
        );
    }

    #[test]
    fn copilot_launch_never_requests_an_rpc_listener() {
        let plan = build_spawn_plan(input(Harness::Copilot)).expect("Copilot native plan");
        assert!(
            !plan
                .args
                .iter()
                .any(|arg| arg == "--ui-server" || arg == "--port")
        );
    }

    #[test]
    fn observed_launch_keeps_discrete_argv_and_names_status_path() {
        let plan = build_spawn_plan(input(Harness::Omp)).expect("harness plan");
        let command = super::observed_launch_command(
            &plan,
            "/opt/pij-rs".to_string(),
            std::path::Path::new("/tmp/spawn.status"),
            std::path::Path::new("/tmp/spawn.log"),
        )
        .expect("observed launch");

        assert_eq!(command.executable, "/opt/pij-rs");
        assert_eq!(
            &command.args[..7],
            [
                "__spawn-child",
                "--status-file",
                "/tmp/spawn.status",
                "--log-file",
                "/tmp/spawn.log",
                "--",
                "env"
            ]
        );
        assert_eq!(&command.args[7..], plan.args);
    }

    #[test]
    fn accept_inbound_is_refused_for_every_non_claude_harness() {
        for harness in [Harness::Pi, Harness::Omp, Harness::Copilot, Harness::Codex] {
            let mut request = input(harness);
            request.accept_inbound = true;
            let error = build_spawn_plan(request).expect_err("must refuse");
            assert!(error.to_string().contains("applies only to claude"));
        }
    }

    #[test]
    fn executable_override_rejects_harness_selectors_and_mismatched_basenames() {
        for executable in ["omp", "/opt/harnesses/omp"] {
            let mut request = input(Harness::Pi);
            request.executable = Some(executable.to_string());
            let error = build_spawn_plan(request).expect_err("cannot disguise omp as pi");
            assert!(error.to_string().contains("use --harness omp"));
        }
        for executable in ["pi", "./bin/pi", ""] {
            let mut request = input(Harness::Pi);
            request.executable = Some(executable.to_string());
            let error = build_spawn_plan(request).expect_err("override must be an absolute path");
            assert!(
                error
                    .to_string()
                    .contains("absolute executable-path override")
            );
        }
    }

    #[test]
    fn absolute_omp_executable_keeps_omp_flag_translation() {
        let mut request = input(Harness::Omp);
        request.executable = Some("/opt/harnesses/omp".to_string());
        let plan = build_spawn_plan(request).expect("omp executable override");
        assert_eq!(
            &plan.args[2..],
            [
                "/opt/harnesses/omp",
                "--auto-approve",
                "--model",
                "provider/model",
                "--thinking",
                "high"
            ]
        );
    }

    #[test]
    fn executable_override_and_absent_optional_facts_are_preserved() {
        let plan = build_spawn_plan(SpawnPlanInput {
            seat_id: SeatId::from("pij-override"),
            spawn_id: "spawn-override".to_string(),
            harness: Harness::Claude,
            executable: Some("/opt/harnesses/claude".to_string()),
            model: None,
            resolved_provider: None,
            effort: None,
            accept_inbound: false,
            resume: None,
        })
        .expect("build plan");

        assert_eq!(plan.executable, "env");
        assert_eq!(
            plan.args,
            [
                "PIJ_SESSION_ID=pij-override",
                "PIJ_SPAWN_ID=spawn-override",
                "/opt/harnesses/claude",
                "--dangerously-skip-permissions"
            ]
        );
        assert_eq!(plan.model, None);
        assert_eq!(plan.provider, None);
        assert_eq!(plan.effort, None);
        assert!(!plan.cross_session_inbound_accept);
    }
}
