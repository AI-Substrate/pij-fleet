use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const INBOUND_KEY: &str = "crossSessionInbound";
const ACCEPT: &str = "accept";
const HOOKS_KEY: &str = "hooks";
const SESSION_START_KEY: &str = "SessionStart";
const USER_PROMPT_SUBMIT_KEY: &str = "UserPromptSubmit";
const STOP_KEY: &str = "Stop";
/// A turn that ends on an API error fires this instead of `Stop` (review N1).
const STOP_FAILURE_KEY: &str = "StopFailure";
const STATUS_LINE_KEY: &str = "statusLine";
const SESSION_START_SCRIPT_NAME: &str = "claude-session-start-pij.sh";
const USER_PROMPT_SUBMIT_SCRIPT_NAME: &str = "claude-user-prompt-submit-pij.sh";
const STOP_SCRIPT_NAME: &str = "claude-stop-pij.sh";
const CLAUDE_SESSION_START_SCRIPT: &str =
    include_str!("../../../harness/scripts/claude-session-start-pij.sh");
const CLAUDE_USER_PROMPT_SUBMIT_SCRIPT: &str =
    include_str!("../../../harness/scripts/claude-user-prompt-submit-pij.sh");
const CLAUDE_STOP_SCRIPT: &str = include_str!("../../../harness/scripts/claude-stop-pij.sh");
const CLAUDE_STATUSLINE_SCRIPT: &str =
    include_str!("../../../harness/scripts/claude-statusline-pij.sh");
const COPILOT_STATUSLINE_SCRIPT: &str =
    include_str!("../../../harness/scripts/copilot-statusline-pij.sh");
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A pure decision about whether and how a Claude settings document must change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnsurePlan {
    /// The document already carries the required value.
    NoChange { before: Option<String> },
    /// Persist these bytes after backing up an existing document.
    Write {
        before: Option<String>,
        contents: String,
    },
    /// The document is not safe to edit.
    Refuse { error: String },
}

/// One inspected Claude home and the action taken there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnsureReport {
    pub home: PathBuf,
    pub before: Option<String>,
    pub after: String,
    pub changed: bool,
    pub backup: Option<PathBuf>,
    pub error: Option<String>,
}

/// Read-only state of one Claude settings file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeInboundState {
    pub home: PathBuf,
    pub file_exists: bool,
    pub current: Option<String>,
    pub error: Option<String>,
}

/// Read-only state of one pij-managed hook (SessionStart, UserPromptSubmit or
/// Stop) in one Claude settings file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeHookState {
    pub home: PathBuf,
    pub file_exists: bool,
    pub installed: bool,
    pub command: Option<String>,
    pub error: Option<String>,
}
/// Read-only state of the managed Claude statusline in one settings file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeStatuslineState {
    pub home: PathBuf,
    pub file_exists: bool,
    pub installed: bool,
    pub command: Option<String>,
    pub error: Option<String>,
}

/// Discover every Claude configuration home this process can govern.
#[must_use]
pub fn claude_homes() -> Vec<PathBuf> {
    claude_homes_from(env::var_os("CLAUDE_CONFIG_DIR"), env::var_os("HOME"))
}

/// Resolve Claude homes from explicit environment values.
#[must_use]
pub fn claude_homes_from(config_dir: Option<OsString>, home: Option<OsString>) -> Vec<PathBuf> {
    let home = home.filter(|value| !value.is_empty()).map(PathBuf::from);
    let mut homes = Vec::with_capacity(2);
    if let Some(config_dir) = config_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        homes.push(config_dir);
    } else if let Some(home) = &home {
        homes.push(home.join(".claude"));
    }
    if let Some(alt) = home.map(|home| home.join(".claude-alt"))
        && alt.exists()
        && !homes.contains(&alt)
    {
        homes.push(alt);
    }
    homes
}

/// Plan the minimal safe edit for one Claude settings document.
#[must_use]
pub fn plan_inbound_accept(existing: Option<&str>) -> EnsurePlan {
    let Some(existing) = existing else {
        return EnsurePlan::Write {
            before: None,
            contents: format!("{{\n  \"{INBOUND_KEY}\": \"{ACCEPT}\"\n}}\n"),
        };
    };
    if existing.trim().is_empty() {
        return EnsurePlan::Write {
            before: None,
            contents: format!("{{\n  \"{INBOUND_KEY}\": \"{ACCEPT}\"\n}}\n"),
        };
    }
    let parsed: Value = match serde_json::from_str(existing) {
        Ok(parsed) => parsed,
        Err(error) => {
            return EnsurePlan::Refuse {
                error: format!("malformed settings.json: {error}"),
            };
        }
    };
    let Some(object) = parsed.as_object() else {
        return EnsurePlan::Refuse {
            error: "settings.json root must be a JSON object".to_string(),
        };
    };
    let before = object.get(INBOUND_KEY).map(render_value);
    if object.get(INBOUND_KEY).and_then(Value::as_str) == Some(ACCEPT) {
        return EnsurePlan::NoChange { before };
    }
    let contents = if object.contains_key(INBOUND_KEY) {
        let Some(range) = find_top_level_value(existing, INBOUND_KEY) else {
            return EnsurePlan::Refuse {
                error: "could not locate crossSessionInbound in valid settings.json".to_string(),
            };
        };
        let mut contents = String::with_capacity(existing.len() + ACCEPT.len());
        contents.push_str(&existing[..range.start]);
        contents
            .push_str(&serde_json::to_string(ACCEPT).expect("string serialization is infallible"));
        contents.push_str(&existing[range.end..]);
        contents
    } else {
        insert_top_level_property(existing, object.is_empty())
    };
    EnsurePlan::Write { before, contents }
}

/// Ensure every named Claude home accepts cross-session inbound messages.
#[must_use]
pub fn ensure_claude_inbound_accept(homes: &[PathBuf]) -> Vec<EnsureReport> {
    homes
        .iter()
        .map(|home| ensure_home(home, ACCEPT, plan_inbound_accept))
        .collect()
}
/// Plan the minimal safe edit that points Claude at pij's managed statusline.
#[must_use]
pub fn plan_claude_statusline(existing: Option<&str>, command: &str) -> EnsurePlan {
    if command.is_empty() {
        return EnsurePlan::Refuse {
            error: "statusline command must not be empty".to_string(),
        };
    }
    let desired = serde_json::json!({"type": "command", "command": command});
    let Some(existing) = existing else {
        return EnsurePlan::Write {
            before: None,
            contents: format!(
                "{{\n  \"{STATUS_LINE_KEY}\": {}\n}}\n",
                serde_json::to_string(&desired).expect("statusline serialization is infallible")
            ),
        };
    };
    if existing.trim().is_empty() {
        return EnsurePlan::Write {
            before: None,
            contents: format!(
                "{{\n  \"{STATUS_LINE_KEY}\": {}\n}}\n",
                serde_json::to_string(&desired).expect("statusline serialization is infallible")
            ),
        };
    }
    let parsed: Value = match serde_json::from_str(existing) {
        Ok(parsed) => parsed,
        Err(error) => {
            return EnsurePlan::Refuse {
                error: format!("malformed settings.json: {error}"),
            };
        }
    };
    let Some(root) = parsed.as_object() else {
        return EnsurePlan::Refuse {
            error: "settings.json root must be a JSON object".to_string(),
        };
    };
    let before = root
        .get(STATUS_LINE_KEY)
        .and_then(Value::as_object)
        .and_then(|statusline| statusline.get("command"))
        .map(render_value);
    if root.get(STATUS_LINE_KEY) == Some(&desired) {
        return EnsurePlan::NoChange { before };
    }
    let rendered = serde_json::to_string(&desired).expect("statusline serialization is infallible");
    let contents = if root.contains_key(STATUS_LINE_KEY) {
        let Some(range) = find_top_level_value(existing, STATUS_LINE_KEY) else {
            return EnsurePlan::Refuse {
                error: "could not locate statusLine in valid settings.json".to_string(),
            };
        };
        replace_range(existing, range, &rendered)
    } else {
        insert_object_property(existing, root.is_empty(), STATUS_LINE_KEY, &rendered)
    };
    EnsurePlan::Write { before, contents }
}

/// Ensure every named Claude home points at pij's managed statusline.
#[must_use]
pub fn ensure_claude_statusline(homes: &[PathBuf], script_path: &Path) -> Vec<EnsureReport> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            let settings_path = home.join("settings.json");
            if let Ok(existing) = fs::read_to_string(&settings_path)
                && let EnsurePlan::Write {
                    before: Some(previous),
                    ..
                } = plan_claude_statusline(Some(&existing), &command)
                && previous != command
                && let Err(error) = backup_statusline_script(home, &previous)
            {
                return report(home, Some(previous), &command, false, None, Some(error));
            }
            ensure_home(home, &command, |existing| {
                plan_claude_statusline(existing, &command)
            })
        })
        .collect()
}

fn backup_statusline_script(home: &Path, command: &str) -> Result<(), String> {
    let path = if let Some(relative) = command.strip_prefix("~/") {
        let Some(root) = home.parent() else {
            return Ok(());
        };
        root.join(relative)
    } else {
        let path = PathBuf::from(command);
        if !path.is_absolute() {
            return Ok(());
        }
        path
    };
    if !path.is_file() {
        return Ok(());
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let backup = path.with_file_name(format!("{name}.bak-{}", unix_millis()));
    fs::copy(&path, &backup).map_err(|error| {
        format!(
            "could not back up prior statusline {} to {}: {error}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}

/// Plan the minimal safe edit that installs pij's Claude SessionStart hook.
#[must_use]
pub fn plan_claude_session_start_hook(existing: Option<&str>, command: &str) -> EnsurePlan {
    plan_claude_hook(
        existing,
        command,
        SESSION_START_KEY,
        SESSION_START_SCRIPT_NAME,
    )
}

/// Plan the minimal safe edit that installs pij's Claude UserPromptSubmit hook,
/// which claims held FYIs into the human's prompt (plan 158).
#[must_use]
pub fn plan_claude_user_prompt_submit_hook(existing: Option<&str>, command: &str) -> EnsurePlan {
    plan_claude_hook(
        existing,
        command,
        USER_PROMPT_SUBMIT_KEY,
        USER_PROMPT_SUBMIT_SCRIPT_NAME,
    )
}

/// Plan the minimal safe edit that installs pij's Claude Stop hook, which
/// publishes the seat idle when Claude finishes its turn (plan 157).
#[must_use]
pub fn plan_claude_stop_hook(existing: Option<&str>, command: &str) -> EnsurePlan {
    plan_claude_hook(existing, command, STOP_KEY, STOP_SCRIPT_NAME)
}

/// Plan the same idle script under `StopFailure`: a turn that ends on an API
/// error (rate limit, auth failure) fires that event and not `Stop`.
#[must_use]
pub fn plan_claude_stop_failure_hook(existing: Option<&str>, command: &str) -> EnsurePlan {
    plan_claude_hook(existing, command, STOP_FAILURE_KEY, STOP_SCRIPT_NAME)
}

/// Install `command` as the single pij-managed entry under `hooks.<event>`.
/// A command is pij-managed when it names `script_name`; foreign entries are
/// preserved byte for byte and stale managed commands are reconciled in place.
fn plan_claude_hook(
    existing: Option<&str>,
    command: &str,
    event: &str,
    script_name: &str,
) -> EnsurePlan {
    if command.is_empty() {
        return EnsurePlan::Refuse {
            error: format!("{event} hook command must not be empty"),
        };
    }
    let entry = serde_json::json!({
        "matcher": "",
        "hooks": [{"type": "command", "command": command}],
    });
    let entry = serde_json::to_string(&entry).expect("hook serialization is infallible");
    let Some(existing) = existing else {
        return EnsurePlan::Write {
            before: None,
            contents: format!("{{\n  \"{HOOKS_KEY}\": {{\n    \"{event}\": [{entry}]\n  }}\n}}\n"),
        };
    };
    if existing.trim().is_empty() {
        return EnsurePlan::Write {
            before: None,
            contents: format!("{{\n  \"{HOOKS_KEY}\": {{\n    \"{event}\": [{entry}]\n  }}\n}}\n"),
        };
    }
    let parsed: Value = match serde_json::from_str(existing) {
        Ok(parsed) => parsed,
        Err(error) => {
            return EnsurePlan::Refuse {
                error: format!("malformed settings.json: {error}"),
            };
        }
    };
    let Some(root) = parsed.as_object() else {
        return EnsurePlan::Refuse {
            error: "settings.json root must be a JSON object".to_string(),
        };
    };
    let Some(hooks) = root.get(HOOKS_KEY) else {
        return EnsurePlan::Write {
            before: None,
            contents: insert_object_property(
                existing,
                root.is_empty(),
                HOOKS_KEY,
                &format!("{{\"{event}\":[{entry}]}}"),
            ),
        };
    };
    let Some(hooks_object) = hooks.as_object() else {
        return EnsurePlan::Refuse {
            error: "settings.json hooks must be a JSON object".to_string(),
        };
    };
    let Some(hooks_range) = find_top_level_value(existing, HOOKS_KEY) else {
        return EnsurePlan::Refuse {
            error: "could not locate hooks in valid settings.json".to_string(),
        };
    };
    let hooks_document = &existing[hooks_range.clone()];
    let Some(event_entries) = hooks_object.get(event) else {
        let updated_hooks = insert_object_property(
            hooks_document,
            hooks_object.is_empty(),
            event,
            &format!("[{entry}]"),
        );
        return EnsurePlan::Write {
            before: None,
            contents: replace_range(existing, hooks_range, &updated_hooks),
        };
    };
    let Some(event_entries) = event_entries.as_array() else {
        return EnsurePlan::Refuse {
            error: format!("settings.json hooks.{event} must be a JSON array"),
        };
    };
    let Some(event_range) = find_top_level_value(hooks_document, event) else {
        return EnsurePlan::Refuse {
            error: format!("could not locate hooks.{event} in valid settings.json"),
        };
    };
    let managed_count = managed_hook_count(event_entries, script_name);
    if managed_count == 1
        && event_entries
            .iter()
            .any(|entry| hook_entry_contains(entry, command))
    {
        return EnsurePlan::NoChange {
            before: Some(command.to_string()),
        };
    }
    if let Some(before) = first_managed_hook_command(event_entries, script_name) {
        let updated_event = reconcile_managed_hook_commands(event_entries, command, script_name);
        let updated_event =
            serde_json::to_string(&updated_event).expect("hook serialization is infallible");
        let updated_hooks = replace_range(hooks_document, event_range, &updated_event);
        return EnsurePlan::Write {
            before: Some(before),
            contents: replace_range(existing, hooks_range, &updated_hooks),
        };
    }
    let updated_event = append_array_value(
        &hooks_document[event_range.clone()],
        event_entries.is_empty(),
        &entry,
    );
    let updated_hooks = replace_range(hooks_document, event_range, &updated_event);
    EnsurePlan::Write {
        before: None,
        contents: replace_range(existing, hooks_range, &updated_hooks),
    }
}
fn first_managed_hook_command(entries: &[Value], script_name: &str) -> Option<String> {
    entries.iter().find_map(|entry| {
        entry.get("hooks")?.as_array()?.iter().find_map(|hook| {
            let command = hook.get("command")?.as_str()?;
            is_managed_hook_command(command, script_name).then(|| command.to_string())
        })
    })
}

fn managed_hook_count(entries: &[Value], script_name: &str) -> usize {
    entries
        .iter()
        .map(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .map_or(0, |hooks| {
                    hooks
                        .iter()
                        .filter(|hook| {
                            hook.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(|command| {
                                    is_managed_hook_command(command, script_name)
                                })
                        })
                        .count()
                })
        })
        .sum()
}

fn reconcile_managed_hook_commands(
    entries: &[Value],
    command: &str,
    script_name: &str,
) -> Vec<Value> {
    let mut kept = false;
    let mut reconciled = Vec::with_capacity(entries.len());
    for original in entries {
        let mut entry = original.clone();
        let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            reconciled.push(entry);
            continue;
        };
        let was_empty = hooks.is_empty();
        hooks.retain_mut(|hook| {
            let managed = hook
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| is_managed_hook_command(command, script_name));
            if !managed {
                return true;
            }
            if kept {
                return false;
            }
            hook["command"] = Value::String(command.to_string());
            kept = true;
            true
        });
        if was_empty || !hooks.is_empty() {
            reconciled.push(entry);
        }
    }
    reconciled
}
fn is_managed_hook_command(command: &str, script_name: &str) -> bool {
    command
        .trim_matches('\'')
        .strip_suffix(script_name)
        .is_some_and(|prefix| prefix.ends_with('/'))
}

/// Ensure every named Claude home invokes pij's SessionStart hook.
#[must_use]
pub fn ensure_claude_session_start_hook(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<EnsureReport> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            ensure_home(home, &command, |existing| {
                plan_claude_session_start_hook(existing, &command)
            })
        })
        .collect()
}

/// Ensure every named Claude home invokes pij's UserPromptSubmit FYI hook.
#[must_use]
pub fn ensure_claude_user_prompt_submit_hook(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<EnsureReport> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            ensure_home(home, &command, |existing| {
                plan_claude_user_prompt_submit_hook(existing, &command)
            })
        })
        .collect()
}

/// Ensure every named Claude home invokes pij's Stop idle hook (plan 157).
#[must_use]
pub fn ensure_claude_stop_hook(homes: &[PathBuf], script_path: &Path) -> Vec<EnsureReport> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            ensure_home(home, &command, |existing| {
                plan_claude_stop_hook(existing, &command)
            })
        })
        .collect()
}

/// Inspect the pij SessionStart hook in every named Claude home.
#[must_use]
pub fn inspect_claude_session_start_hook(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<ClaudeHookState> {
    inspect_claude_hook(homes, script_path, plan_claude_session_start_hook)
}

/// Inspect the pij UserPromptSubmit FYI hook in every named Claude home.
#[must_use]
pub fn inspect_claude_user_prompt_submit_hook(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<ClaudeHookState> {
    inspect_claude_hook(homes, script_path, plan_claude_user_prompt_submit_hook)
}

/// Ensure every named Claude home also runs the idle script on `StopFailure`.
#[must_use]
pub fn ensure_claude_stop_failure_hook(homes: &[PathBuf], script_path: &Path) -> Vec<EnsureReport> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            ensure_home(home, &command, |existing| {
                plan_claude_stop_failure_hook(existing, &command)
            })
        })
        .collect()
}

/// Inspect the pij `StopFailure` idle hook in every named Claude home.
#[must_use]
pub fn inspect_claude_stop_failure_hook(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<ClaudeHookState> {
    inspect_claude_hook(homes, script_path, plan_claude_stop_failure_hook)
}

/// Inspect the pij Stop idle hook in every named Claude home (plan 157).
#[must_use]
pub fn inspect_claude_stop_hook(homes: &[PathBuf], script_path: &Path) -> Vec<ClaudeHookState> {
    inspect_claude_hook(homes, script_path, plan_claude_stop_hook)
}

fn inspect_claude_hook(
    homes: &[PathBuf],
    script_path: &Path,
    plan: fn(Option<&str>, &str) -> EnsurePlan,
) -> Vec<ClaudeHookState> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            let path = home.join("settings.json");
            match fs::read_to_string(&path) {
                Ok(existing) => match plan(Some(&existing), &command) {
                    EnsurePlan::NoChange { before } => ClaudeHookState {
                        home: home.clone(),
                        file_exists: true,
                        installed: true,
                        command: before,
                        error: None,
                    },
                    EnsurePlan::Write { before, .. } => ClaudeHookState {
                        home: home.clone(),
                        file_exists: true,
                        installed: false,
                        command: before,
                        error: None,
                    },
                    EnsurePlan::Refuse { error } => ClaudeHookState {
                        home: home.clone(),
                        file_exists: true,
                        installed: false,
                        command: None,
                        error: Some(error),
                    },
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ClaudeHookState {
                    home: home.clone(),
                    file_exists: false,
                    installed: false,
                    command: None,
                    error: None,
                },
                Err(error) => ClaudeHookState {
                    home: home.clone(),
                    file_exists: true,
                    installed: false,
                    command: None,
                    error: Some(format!("could not read {}: {error}", path.display())),
                },
            }
        })
        .collect()
}

/// Materialize the embedded Claude hook at a stable daemon-owned path.
pub fn install_claude_session_start_script(state_dir: &Path) -> std::io::Result<PathBuf> {
    install_script(
        state_dir,
        SESSION_START_SCRIPT_NAME,
        CLAUDE_SESSION_START_SCRIPT,
    )
}

/// Materialize the embedded Claude UserPromptSubmit FYI hook at a stable
/// daemon-owned path (plan 158).
pub fn install_claude_user_prompt_submit_script(state_dir: &Path) -> std::io::Result<PathBuf> {
    install_script(
        state_dir,
        USER_PROMPT_SUBMIT_SCRIPT_NAME,
        CLAUDE_USER_PROMPT_SUBMIT_SCRIPT,
    )
}

/// Materialize the embedded Claude Stop idle hook at a stable daemon-owned
/// path (plan 157).
pub fn install_claude_stop_script(state_dir: &Path) -> std::io::Result<PathBuf> {
    install_script(state_dir, STOP_SCRIPT_NAME, CLAUDE_STOP_SCRIPT)
}

fn install_script(state_dir: &Path, name: &str, contents: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(state_dir)?;
    let path = state_dir.join(name);
    if fs::read_to_string(&path).ok().as_deref() != Some(contents) {
        atomic_replace(&path, contents.as_bytes())?;
    }
    make_executable(&path)?;
    Ok(path)
}
/// Inspect pij's managed Claude statusline in every named home.
#[must_use]
pub fn inspect_claude_statusline(
    homes: &[PathBuf],
    script_path: &Path,
) -> Vec<ClaudeStatuslineState> {
    let command = shell_command(script_path);
    homes
        .iter()
        .map(|home| {
            let path = home.join("settings.json");
            match fs::read_to_string(&path) {
                Ok(existing) => match plan_claude_statusline(Some(&existing), &command) {
                    EnsurePlan::NoChange { before } => ClaudeStatuslineState {
                        home: home.clone(),
                        file_exists: true,
                        installed: true,
                        command: before,
                        error: None,
                    },
                    EnsurePlan::Write { before, .. } => ClaudeStatuslineState {
                        home: home.clone(),
                        file_exists: true,
                        installed: false,
                        command: before,
                        error: None,
                    },
                    EnsurePlan::Refuse { error } => ClaudeStatuslineState {
                        home: home.clone(),
                        file_exists: true,
                        installed: false,
                        command: None,
                        error: Some(error),
                    },
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ClaudeStatuslineState {
                        home: home.clone(),
                        file_exists: false,
                        installed: false,
                        command: None,
                        error: None,
                    }
                }
                Err(error) => ClaudeStatuslineState {
                    home: home.clone(),
                    file_exists: true,
                    installed: false,
                    command: None,
                    error: Some(format!("could not read {}: {error}", path.display())),
                },
            }
        })
        .collect()
}

/// Materialize the embedded statusline at a stable daemon-owned path.
pub fn install_claude_statusline_script(state_dir: &Path) -> std::io::Result<PathBuf> {
    install_script(
        state_dir,
        "claude-statusline-pij.sh",
        CLAUDE_STATUSLINE_SCRIPT,
    )
}

/// Materialize pij's Copilot statusline at a stable daemon-owned path (plan 156).
pub fn install_copilot_statusline_script(state_dir: &Path) -> std::io::Result<PathBuf> {
    install_script(
        state_dir,
        "copilot-statusline-pij.sh",
        COPILOT_STATUSLINE_SCRIPT,
    )
}

/// The Copilot CLI home (`$COPILOT_HOME`, else `~/.copilot`), when it exists.
#[must_use]
pub fn copilot_home() -> Option<PathBuf> {
    env::var_os("COPILOT_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".copilot")))
        .filter(|home| home.is_dir())
}

/// Point Copilot's `settings.json` `statusLine` at pij's script. Copilot uses the
/// same key and shape as Claude, so this is the same minimal, backed-up edit.
#[must_use]
pub fn ensure_copilot_statusline(home: &Path, script_path: &Path) -> Vec<EnsureReport> {
    ensure_claude_statusline(&[home.to_path_buf()], script_path)
}

/// Inspect every named Claude home without changing any file.
#[must_use]
pub fn inspect_claude_inbound(homes: &[PathBuf]) -> Vec<ClaudeInboundState> {
    homes
        .iter()
        .map(|home| {
            let path = home.join("settings.json");
            match fs::read_to_string(&path) {
                Ok(existing) => match plan_inbound_accept(Some(&existing)) {
                    EnsurePlan::NoChange { before } | EnsurePlan::Write { before, .. } => {
                        ClaudeInboundState {
                            home: home.clone(),
                            file_exists: true,
                            current: before,
                            error: None,
                        }
                    }
                    EnsurePlan::Refuse { error } => ClaudeInboundState {
                        home: home.clone(),
                        file_exists: true,
                        current: None,
                        error: Some(error),
                    },
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ClaudeInboundState {
                    home: home.clone(),
                    file_exists: false,
                    current: None,
                    error: None,
                },
                Err(error) => ClaudeInboundState {
                    home: home.clone(),
                    file_exists: true,
                    current: None,
                    error: Some(format!("could not read {}: {error}", path.display())),
                },
            }
        })
        .collect()
}

fn ensure_home<F>(home: &Path, after: &str, plan: F) -> EnsureReport
where
    F: FnOnce(Option<&str>) -> EnsurePlan,
{
    let path = home.join("settings.json");
    let existing = match fs::read_to_string(&path) {
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return report(
                home,
                None,
                after,
                false,
                None,
                Some(format!("could not read {}: {error}", path.display())),
            );
        }
    };
    match plan(existing.as_deref()) {
        EnsurePlan::NoChange { before } => report(home, before, after, false, None, None),
        EnsurePlan::Refuse { error } => report(home, None, after, false, None, Some(error)),
        EnsurePlan::Write { before, contents } => {
            if let Err(error) = fs::create_dir_all(home) {
                return report(
                    home,
                    before,
                    after,
                    false,
                    None,
                    Some(format!("could not create {}: {error}", home.display())),
                );
            }
            let backup = if existing.is_some() {
                let backup = home.join(format!("settings.json.bak-{}", unix_millis()));
                if let Err(error) = fs::copy(&path, &backup) {
                    return report(
                        home,
                        before,
                        after,
                        false,
                        None,
                        Some(format!(
                            "could not back up {} to {}: {error}",
                            path.display(),
                            backup.display()
                        )),
                    );
                }
                Some(backup)
            } else {
                None
            };
            if let Err(error) = atomic_replace(&path, contents.as_bytes()) {
                return report(
                    home,
                    before,
                    after,
                    false,
                    backup,
                    Some(format!(
                        "could not write {} atomically: {error}",
                        path.display()
                    )),
                );
            }
            report(home, before, after, true, backup, None)
        }
    }
}
fn report(
    home: &Path,
    before: Option<String>,
    after: &str,
    changed: bool,
    backup: Option<PathBuf>,
    error: Option<String>,
) -> EnsureReport {
    EnsureReport {
        home: home.to_path_buf(),
        before,
        after: after.to_string(),
        changed,
        backup,
        error,
    }
}
fn insert_top_level_property(document: &str, empty: bool) -> String {
    let value = serde_json::to_string(ACCEPT).expect("string serialization is infallible");
    insert_object_property(document, empty, INBOUND_KEY, &value)
}

fn insert_object_property(document: &str, empty: bool, key: &str, value: &str) -> String {
    let closing = document
        .rfind('}')
        .expect("validated JSON object always has a closing brace");
    let before_closing = &document[..closing];
    let content_end = before_closing.trim_end().len();
    let multiline = before_closing.contains('\n');
    let line_break = if document.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output = String::with_capacity(document.len() + key.len() + value.len() + 10);
    output.push_str(&document[..content_end]);
    if empty {
        if multiline {
            output.push_str(&format!("{line_break}  \"{key}\": {value}"));
        } else {
            output.push_str(&format!("\"{key}\":{value}"));
        }
    } else if multiline {
        output.push_str(&format!(",{line_break}  \"{key}\": {value}"));
    } else {
        output.push_str(&format!(",\"{key}\":{value}"));
    }
    output.push_str(&document[content_end..]);
    output
}

fn append_array_value(document: &str, empty: bool, value: &str) -> String {
    let closing = document
        .rfind(']')
        .expect("validated JSON array always has a closing bracket");
    let content_end = document[..closing].trim_end().len();
    let multiline = document[..closing].contains('\n');
    let line_break = if document.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut output = String::with_capacity(document.len() + value.len() + 4);
    output.push_str(&document[..content_end]);
    if !empty {
        output.push(',');
    }
    if multiline {
        output.push_str(line_break);
        output.push_str("  ");
    }
    output.push_str(value);
    output.push_str(&document[content_end..]);
    output
}

fn replace_range(document: &str, range: Range<usize>, replacement: &str) -> String {
    let mut output = String::with_capacity(document.len() + replacement.len());
    output.push_str(&document[..range.start]);
    output.push_str(replacement);
    output.push_str(&document[range.end..]);
    output
}

fn hook_entry_contains(entry: &Value, command: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|hook| hook.get("command").and_then(Value::as_str) == Some(command))
        })
}

fn shell_command(path: &Path) -> String {
    let path = path.to_string_lossy();
    if path.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    }) {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\"'\"'"))
    }
}

fn atomic_replace(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = path.with_file_name(format!(".{name}.pij-{}-{sequence}.tmp", std::process::id()));
    let result = (|| {
        let permissions = fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Some(permissions) = permissions {
            fs::set_permissions(&temp, permissions)?;
        }
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o700);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn render_value(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn find_top_level_value(document: &str, wanted: &str) -> Option<Range<usize>> {
    let bytes = document.as_bytes();
    let mut cursor = skip_whitespace(bytes, 0);
    if bytes.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor += 1;
    let mut found = None;
    loop {
        cursor = skip_whitespace(bytes, cursor);
        if bytes.get(cursor) == Some(&b'}') {
            return found;
        }
        let key_start = cursor;
        let key_end = string_end(bytes, key_start)?;
        let key: String = serde_json::from_str(&document[key_start..key_end]).ok()?;
        cursor = skip_whitespace(bytes, key_end);
        if bytes.get(cursor) != Some(&b':') {
            return None;
        }
        cursor = skip_whitespace(bytes, cursor + 1);
        let value_start = cursor;
        let mut stream =
            serde_json::Deserializer::from_str(&document[value_start..]).into_iter::<Value>();
        stream.next()?.ok()?;
        let value_end = value_start + stream.byte_offset();
        if key == wanted {
            found = Some(value_start..value_end);
        }
        cursor = skip_whitespace(bytes, value_end);
        match bytes.get(cursor) {
            Some(b',') => cursor += 1,
            Some(b'}') => return found,
            _ => return None,
        }
    }
}

fn skip_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while let Some(byte) = bytes.get(cursor) {
        match byte {
            b'\\' => cursor += 2,
            b'"' => return Some(cursor + 1),
            _ => cursor += 1,
        }
    }
    None
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    /// Exec of a script this test just wrote can fail with ETXTBSY on Linux when
    /// a sibling test's forked child still holds the write fd between fork and
    /// exec (CLOEXEC closes it only at exec). Seen on CI (PR #371). The window is
    /// microseconds; retry briefly. Production never execs a file it just wrote.
    fn retry_etxtbsy<T>(mut run: impl FnMut() -> std::io::Result<T>) -> T {
        for _ in 0..100 {
            match run() {
                Err(error) if error.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                other => return other.expect("run hook"),
            }
        }
        panic!("run hook: ETXTBSY persisted");
    }
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("pij-claude-settings-{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn absent_file_is_created_with_accept() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        let reports = ensure_claude_inbound_accept(std::slice::from_ref(&home));
        assert_eq!(reports.len(), 1);
        assert!(reports[0].changed);
        assert_eq!(reports[0].before, None);
        assert_eq!(reports[0].after, "accept");
        assert_eq!(reports[0].backup, None);
        assert_eq!(reports[0].error, None);
        let written = fs::read_to_string(home.join("settings.json")).expect("settings created");
        assert_eq!(
            serde_json::from_str::<Value>(&written).expect("valid settings")[INBOUND_KEY],
            ACCEPT
        );
    }

    #[test]
    fn absent_key_is_added_without_rewriting_other_settings() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{\n  \"theme\": \"dark\",\n  \"nested\": { \"keep\": true }\n}\n";
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let report = ensure_claude_inbound_accept(std::slice::from_ref(&home))
            .into_iter()
            .next()
            .expect("report");
        assert!(report.changed);
        assert_eq!(report.before, None);
        assert_eq!(
            fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
            original
        );
        let written = fs::read_to_string(home.join("settings.json")).expect("settings readable");
        assert!(written.contains("  \"theme\": \"dark\""));
        assert!(written.contains("  \"nested\": { \"keep\": true }"));
        assert_eq!(
            serde_json::from_str::<Value>(&written).expect("valid settings")[INBOUND_KEY],
            ACCEPT
        );
    }

    #[test]
    fn hold_is_replaced_and_original_is_backed_up() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{\"before\":1,\"crossSessionInbound\":\"hold\",\"after\":2}";
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let report = ensure_claude_inbound_accept(std::slice::from_ref(&home))
            .into_iter()
            .next()
            .expect("report");
        assert!(report.changed);
        assert_eq!(report.before.as_deref(), Some("hold"));
        assert_eq!(
            fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
            original
        );
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings readable"),
            "{\"before\":1,\"crossSessionInbound\":\"accept\",\"after\":2}"
        );
    }

    #[test]
    fn already_accept_is_a_no_op_without_backup() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{\"crossSessionInbound\": \"accept\", \"keep\": true}";
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let report = ensure_claude_inbound_accept(std::slice::from_ref(&home))
            .into_iter()
            .next()
            .expect("report");
        assert!(!report.changed);
        assert_eq!(report.before.as_deref(), Some(ACCEPT));
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings readable"),
            original
        );
    }

    #[test]
    fn malformed_json_is_untouched_and_reported() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{not-json";
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let report = ensure_claude_inbound_accept(std::slice::from_ref(&home))
            .into_iter()
            .next()
            .expect("report");
        assert!(!report.changed);
        assert!(report.error.is_some());
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings readable"),
            original
        );
    }

    #[test]
    fn empty_and_whitespace_files_are_backed_up_then_repaired() {
        let root = TestDir::new();
        for (name, original) in [("empty", ""), ("whitespace", "  \n\t")] {
            let home = root.path().join(name);
            fs::create_dir_all(&home).expect("home");
            fs::write(home.join("settings.json"), original).expect("seed settings");
            let report = ensure_claude_inbound_accept(std::slice::from_ref(&home))
                .into_iter()
                .next()
                .expect("report");
            assert!(report.changed);
            assert_eq!(
                fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
                original
            );
            let written = fs::read_to_string(home.join("settings.json")).expect("settings");
            assert_eq!(
                serde_json::from_str::<Value>(&written).expect("valid")[INBOUND_KEY],
                ACCEPT
            );
        }
    }

    #[test]
    fn insertion_preserves_crlf_line_endings() {
        let EnsurePlan::Write { contents, .. } =
            plan_inbound_accept(Some("{\r\n  \"keep\": true\r\n}\r\n"))
        else {
            panic!("missing key must change");
        };
        assert!(!contents.replace("\r\n", "").contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&contents).expect("valid")[INBOUND_KEY],
            ACCEPT
        );
    }

    #[test]
    fn duplicate_keys_update_the_effective_last_value() {
        let EnsurePlan::Write { contents, .. } = plan_inbound_accept(Some(
            r#"{"crossSessionInbound":"accept","crossSessionInbound":false}"#,
        )) else {
            panic!("effective false value must change");
        };
        let parsed: Value = serde_json::from_str(&contents).expect("valid settings");
        assert_eq!(parsed[INBOUND_KEY], ACCEPT);
        assert!(contents.ends_with(r#""crossSessionInbound":"accept"}"#));
    }

    #[test]
    fn pure_plan_refuses_non_object_json() {
        assert!(matches!(
            plan_inbound_accept(Some("[]")),
            EnsurePlan::Refuse { .. }
        ));
    }

    #[test]
    fn home_discovery_prefers_config_dir_and_includes_existing_alt() {
        let root = TestDir::new();
        let configured = root.path().join("custom-claude");
        let alt = root.path().join(".claude-alt");
        fs::create_dir_all(&alt).expect("alt home");
        let homes = claude_homes_from(Some(configured.clone().into()), Some(root.path().into()));
        assert_eq!(homes, vec![configured, alt]);
    }

    #[test]
    fn session_start_hook_creates_missing_settings() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        let script = Path::new("/opt/pij/claude-session-start-pij.sh");

        let report = ensure_claude_session_start_hook(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(report.changed);
        assert_eq!(report.before, None);
        assert_eq!(report.after, script.display().to_string());
        assert_eq!(report.backup, None);
        assert_eq!(report.error, None);
        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings created"),
        )
        .expect("valid settings");
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            script.display().to_string()
        );
    }

    #[test]
    fn session_start_hook_shell_quotes_spaced_script_paths() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        let script = Path::new("/opt/pij hooks/session-start.sh");

        let _ = ensure_claude_session_start_hook(std::slice::from_ref(&home), script);

        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings created"),
        )
        .expect("valid settings");
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "'/opt/pij hooks/session-start.sh'"
        );
    }

    #[test]
    fn session_start_hook_preserves_existing_hooks_byte_for_byte() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = concat!(
            "{\n",
            "  \"hooks\": {\n",
            "    \"Notification\": [{\"matcher\":\"before\",\"hooks\":[{\"type\":\"command\",\"command\":\"notify\"}]}],\n",
            "    \"PreToolUse\": [{\"matcher\":\"tool\",\"hooks\":[{\"type\":\"command\",\"command\":\"pre\"}]}],\n",
            "    \"PostToolUse\": [{\"matcher\":\"after\",\"hooks\":[{\"type\":\"command\",\"command\":\"post\"}]}]\n",
            "  },\n",
            "  \"theme\": \"dark\"\n",
            "}\n"
        );
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let script = Path::new("/opt/pij/claude-session-start-pij.sh");

        let report = ensure_claude_session_start_hook(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(report.changed);
        assert_eq!(
            fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
            original
        );
        let written = fs::read_to_string(home.join("settings.json")).expect("settings");
        for preserved in [
            r#""Notification": [{"matcher":"before","hooks":[{"type":"command","command":"notify"}]}]"#,
            r#""PreToolUse": [{"matcher":"tool","hooks":[{"type":"command","command":"pre"}]}]"#,
            r#""PostToolUse": [{"matcher":"after","hooks":[{"type":"command","command":"post"}]}]"#,
        ] {
            assert!(
                written.contains(preserved),
                "existing hook changed: {preserved}"
            );
        }
        let parsed: Value = serde_json::from_str(&written).expect("valid settings");
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(
            parsed["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            script.display().to_string()
        );
    }

    #[test]
    fn existing_session_start_hook_is_a_no_op_without_backup() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let script = Path::new("/opt/pij/claude-session-start-pij.sh");
        let original = format!(
            r#"{{"hooks":{{"SessionStart":[{{"matcher":"","hooks":[{{"type":"command","command":{}}}]}}]}}}}"#,
            serde_json::to_string(&script.display().to_string()).expect("path serialization")
        );
        fs::write(home.join("settings.json"), &original).expect("seed settings");

        let report = ensure_claude_session_start_hook(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(!report.changed);
        assert_eq!(report.before.as_deref(), script.to_str());
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings"),
            original
        );
    }

    #[test]
    fn session_start_hook_appends_without_reordering_existing_entries() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let existing_entry =
            r#"{"matcher":"resume","hooks":[{"type":"command","command":"existing"}]}"#;
        let original = format!(r#"{{"hooks":{{"SessionStart":[{existing_entry}]}}}}"#);
        fs::write(home.join("settings.json"), &original).expect("seed settings");
        let before = inspect_claude_session_start_hook(
            std::slice::from_ref(&home),
            Path::new("/opt/pij/claude-session-start-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("inspection");
        assert!(!before.installed);
        assert_eq!(before.command, None);

        let report = ensure_claude_session_start_hook(
            std::slice::from_ref(&home),
            Path::new("/opt/pij/claude-session-start-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(report.changed);
        let written = fs::read_to_string(home.join("settings.json")).expect("settings");
        assert!(written.contains(&format!("[{existing_entry},")));
        let parsed: Value = serde_json::from_str(&written).expect("valid settings");
        assert_eq!(
            parsed["hooks"]["SessionStart"]
                .as_array()
                .expect("array")
                .len(),
            2
        );
        assert_eq!(
            parsed["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "existing"
        );
    }

    #[test]
    fn stale_managed_hook_is_replaced_in_place() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/tmp/old/claude-session-start-pij.sh"}]},{"matcher":"resume","hooks":[{"type":"command","command":"existing"}]}]}}"#;
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report = ensure_claude_session_start_hook(
            std::slice::from_ref(&home),
            Path::new("/Users/test/.pij-rs/claude-session-start-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(report.changed);
        assert_eq!(
            report.before.as_deref(),
            Some("/tmp/old/claude-session-start-pij.sh")
        );
        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings"),
        )
        .expect("valid settings");
        let entries = written["hooks"]["SessionStart"].as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            "/Users/test/.pij-rs/claude-session-start-pij.sh"
        );
        assert_eq!(entries[1]["hooks"][0]["command"], "existing");
    }

    #[test]
    fn duplicate_stale_managed_hooks_collapse_to_one() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/tmp/a/claude-session-start-pij.sh"}]},{"matcher":"resume","hooks":[{"type":"command","command":"foreign"}]},{"matcher":"","hooks":[{"type":"command","command":"/tmp/b/claude-session-start-pij.sh"}]}]}}"#;
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let script = Path::new("/Users/test/.pij-rs/claude-session-start-pij.sh");

        let report = ensure_claude_session_start_hook(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(report.changed);
        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings"),
        )
        .expect("valid settings");
        let entries = written["hooks"]["SessionStart"].as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            script.display().to_string()
        );
        assert_eq!(entries[1]["hooks"][0]["command"], "foreign");
        assert!(matches!(
            plan_claude_session_start_hook(
                Some(&fs::read_to_string(home.join("settings.json")).expect("settings")),
                &script.display().to_string()
            ),
            EnsurePlan::NoChange { .. }
        ));
    }

    #[test]
    fn correct_managed_hook_removes_stale_sibling() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let desired = "/Users/test/.pij-rs/claude-session-start-pij.sh";
        let original = format!(
            r#"{{"hooks":{{"SessionStart":[{{"matcher":"","hooks":[{{"type":"command","command":"{desired}"}}]}},{{"matcher":"","hooks":[{{"type":"command","command":"/tmp/stale/claude-session-start-pij.sh"}}]}}]}}}}"#
        );
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report =
            ensure_claude_session_start_hook(std::slice::from_ref(&home), Path::new(desired))
                .into_iter()
                .next()
                .expect("report");

        assert!(report.changed);
        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings"),
        )
        .expect("valid settings");
        let entries = written["hooks"]["SessionStart"].as_array().expect("array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["hooks"][0]["command"], desired);
    }

    #[test]
    fn foreign_empty_hook_entry_survives_reconciliation() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = r#"{"hooks":{"SessionStart":[{"matcher":"resume","hooks":[]},{"matcher":"","hooks":[{"type":"command","command":"/tmp/stale/claude-session-start-pij.sh"}]}]}}"#;
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report = ensure_claude_session_start_hook(
            std::slice::from_ref(&home),
            Path::new("/Users/test/.pij-rs/claude-session-start-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(report.changed);
        let written: Value = serde_json::from_str(
            &fs::read_to_string(home.join("settings.json")).expect("settings"),
        )
        .expect("valid settings");
        let entries = written["hooks"]["SessionStart"].as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["matcher"], "resume");
        assert_eq!(entries[0]["hooks"], serde_json::json!([]));
    }

    #[test]
    fn session_start_hook_refuses_malformed_settings_without_touching_them() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{not-json";
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report = ensure_claude_session_start_hook(
            std::slice::from_ref(&home),
            Path::new("/opt/pij/claude-session-start-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(!report.changed);
        assert!(report.error.is_some());
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings"),
            original
        );
    }

    #[test]
    fn statusline_command_is_replaced_without_touching_hooks() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = concat!(
            "{\n",
            "  \"statusLine\": {\"type\":\"command\",\"command\":\"~/.claude/statusline-context.sh\"},\n",
            "  \"hooks\": {\"SessionStart\":[{\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"keep\"}]}]}\n",
            "}\n"
        );
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let previous_script = home.join("statusline-context.sh");
        fs::write(&previous_script, "#!/bin/sh\nprintf old\n").expect("seed statusline");
        let script = Path::new("/Users/test/.pij-rs/claude-statusline-pij.sh");

        let report = ensure_claude_statusline(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(report.changed);
        assert_eq!(
            report.before.as_deref(),
            Some("~/.claude/statusline-context.sh")
        );
        assert_eq!(
            fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
            original
        );
        assert_eq!(
            fs::read_to_string(&previous_script).expect("previous script"),
            "#!/bin/sh\nprintf old\n"
        );
        let script_backups = fs::read_dir(&home)
            .expect("read home")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("statusline-context.sh.bak-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(script_backups.len(), 1);
        assert_eq!(
            fs::read_to_string(&script_backups[0]).expect("script backup"),
            "#!/bin/sh\nprintf old\n"
        );
        let written = fs::read_to_string(home.join("settings.json")).expect("settings");
        assert!(written.contains(r#""hooks": {"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"keep"}]}]}"#));
        let parsed: Value = serde_json::from_str(&written).expect("valid settings");
        assert_eq!(parsed["statusLine"]["type"], "command");
        assert_eq!(
            parsed["statusLine"]["command"],
            script.display().to_string()
        );
    }

    #[test]
    fn correct_statusline_command_is_a_no_op_without_backup() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let script = Path::new("/Users/test/.pij-rs/claude-statusline-pij.sh");
        let original = format!(
            r#"{{\"statusLine\":{{\"type\":\"command\",\"command\":{}}},\"keep\":true}}"#,
            serde_json::to_string(&script.display().to_string()).expect("path serialization")
        );
        fs::write(home.join("settings.json"), &original).expect("seed settings");

        let report = ensure_claude_statusline(std::slice::from_ref(&home), script)
            .into_iter()
            .next()
            .expect("report");

        assert!(!report.changed);
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings"),
            original
        );
    }

    #[test]
    fn statusline_refuses_malformed_settings_without_touching_them() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = "{not-json";
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report = ensure_claude_statusline(
            std::slice::from_ref(&home),
            Path::new("/Users/test/.pij-rs/claude-statusline-pij.sh"),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(!report.changed);
        assert!(report.error.is_some());
        assert_eq!(report.backup, None);
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).expect("settings"),
            original
        );
    }

    #[cfg(unix)]
    /// Waits for a statusline run with a hard deadline: a hung script must fail
    /// the test in seconds, never stall a whole CI job (main, 2026-09-10).
    fn wait_bounded(
        mut child: std::process::Child,
        deadline: std::time::Duration,
    ) -> std::process::Output {
        use std::io::Read as _;
        let mut stdout = child.stdout.take().expect("stdout");
        let reader = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stdout.read_to_end(&mut buffer);
            buffer
        });
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("wait for statusline") {
                let stdout = reader.join().expect("stdout reader");
                return std::process::Output {
                    status,
                    stdout,
                    stderr: Vec::new(),
                };
            }
            if started.elapsed() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = reader.join().expect("stdout reader");
                panic!(
                    "statusline script exceeded {deadline:?}; stdout={}",
                    String::from_utf8_lossy(&stdout)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn statusline_script_terminates_without_jq_on_path() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        // No jq on PATH: the payload cannot be parsed, CWD falls back to `.`, and
        // the git walk must still terminate. Before the fix `${PROBE%/*}` left `.`
        // unchanged and the loop never ended (Linux CI rust job, 2026-09-10).
        let root = TestDir::new();
        let state_dir = root.path().join("state");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("bin directory");
        let rs = bin_dir.join("pij-rs");
        fs::write(
            &rs,
            "#!/bin/sh\nprintf '{\"ok\":true,\"data\":{\"id\":\"pij-rs-seat\"}}\\n'\n",
        )
        .expect("fake rs");
        let mut permissions = fs::metadata(&rs).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&rs, permissions).expect("executable fake");
        let script = install_claude_statusline_script(&state_dir).expect("install script");
        let mut hook = std::process::Command::new(&script);
        hook.env("PATH", bin_dir.display().to_string())
            .env("HOME", root.path())
            .env("TMUX_PANE", "%42")
            .current_dir(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = retry_etxtbsy(|| hook.spawn());
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"{}")
            .expect("write input");
        let output = wait_bounded(child, std::time::Duration::from_secs(10));
        assert!(output.status.success(), "status {}", output.status);
        let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
        assert!(stdout.contains("pij-rs-seat"), "stdout={stdout}");
    }

    #[test]
    fn statusline_script_prefers_rs_and_falls_back_to_legacy() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let root = TestDir::new();
        let state_dir = root.path().join("state");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("bin directory");
        let rs = bin_dir.join("pij-rs");
        let legacy = bin_dir.join("pij");
        let legacy_marker = root.path().join("legacy-called");
        fs::write(
            &rs,
            concat!(
                "#!/bin/sh\n",
                "if [ -n \"${PIJ_SESSION_ID:-}\" ]; then\n",
                "  printf '{\"ok\":false,\"error\":\"refused\"}\\n'\n",
                "  exit 2\n",
                "fi\n",
                "printf '{\"ok\":true,\"data\":{\"id\":\"pij-rs-seat\"}}\\n'\n"
            ),
        )
        .expect("fake rs");
        fs::write(
            &legacy,
            "#!/bin/sh\nprintf called >\"$LEGACY_MARKER\"\nprintf 'pij session: pij-legacy-seat\\n'\n",
        )
        .expect("fake legacy");
        for path in [&rs, &legacy] {
            let mut permissions = fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).expect("executable fake");
        }
        let script = install_claude_statusline_script(&state_dir).expect("install script");
        let input = format!(
            concat!(
                r#"{{"session_id":"fixture","transcript_path":{},"cwd":{},"effort":{{"level":"high"}},"model":{{"id":"claude-opus-5[1m]","display_name":"Opus 5 (1M context)"}},"workspace":{{"current_dir":{},"project_dir":{}}},"context_window":{{"total_input_tokens":46133,"total_output_tokens":4,"context_window_size":1000000,"used_percentage":5,"remaining_percentage":95}},"rate_limits":{{"five_hour":{{"used_percentage":29}}}}}}"#
            ),
            serde_json::to_string(&root.path().join("missing.jsonl").display().to_string())
                .expect("transcript serialization"),
            serde_json::to_string(&root.path().display().to_string()).expect("cwd serialization"),
            serde_json::to_string(&root.path().display().to_string())
                .expect("workspace serialization"),
            serde_json::to_string(&root.path().display().to_string())
                .expect("project serialization")
        );
        let run = || {
            let mut hook = std::process::Command::new(&script);
            // jq must be reachable wherever the host installs it (runner images
            // moved it out of /usr/bin once); the fakes still shadow pij binaries.
            let jq_dir = std::env::var_os("PATH")
                .and_then(|path| std::env::split_paths(&path).find(|dir| dir.join("jq").is_file()))
                .map(|dir| format!(":{}", dir.display()))
                .unwrap_or_default();
            hook.env(
                "PATH",
                format!("{}{jq_dir}:/usr/bin:/bin", bin_dir.display()),
            )
            .env("HOME", root.path())
            .env("TMUX_PANE", "%42")
            .env("PIJ_SESSION_ID", "pij-other-seat")
            .env("LEGACY_MARKER", &legacy_marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
            let mut child = retry_etxtbsy(|| hook.spawn());
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input.as_bytes())
                .expect("write input");
            wait_bounded(child, std::time::Duration::from_secs(10))
        };

        let rs_output = run();
        assert!(rs_output.status.success());
        let rs_stdout = String::from_utf8(rs_output.stdout).expect("utf-8 stdout");
        assert!(rs_stdout.contains("\u{1b}[33mrs\u{1b}[0m \u{1b}[34m⛓ pij-rs-seat\u{1b}[0m"));
        for segment in ["Opus 5 (1M context)", "high", "50k/1.0M", "29%"] {
            assert!(rs_stdout.contains(segment), "rs output lost {segment}");
        }
        assert!(!legacy_marker.exists(), "legacy CLI ran after rs answered");

        fs::write(&rs, "#!/bin/sh\nexit 1\n").expect("rs miss");
        let legacy_output = run();
        assert!(legacy_output.status.success());
        let legacy_stdout = String::from_utf8(legacy_output.stdout).expect("utf-8 stdout");
        assert!(
            legacy_stdout.contains("\u{1b}[2mlegacy\u{1b}[0m \u{1b}[34m⛓ pij-legacy-seat\u{1b}[0m")
        );
        for segment in ["Opus 5 (1M context)", "high", "50k/1.0M", "29%"] {
            assert!(
                legacy_stdout.contains(segment),
                "legacy fallback lost {segment}"
            );
        }
        assert!(
            legacy_marker.exists(),
            "legacy CLI did not run after rs miss"
        );
    }

    #[cfg(unix)]
    /// Hook lines lead with an RFC 3339 UTC stamp (plan 156); assert it, drop it.
    fn unstamped(log: &str) -> String {
        log.lines()
            .map(|line| {
                let (stamp, rest) = line.split_once(' ').expect("stamped line");
                assert!(
                    stamp.len() == 20 && stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
                    "{line}"
                );
                format!("{rest}\n")
            })
            .collect()
    }

    #[test]
    fn installed_script_is_executable_and_silent_without_tmux() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new();
        let state_dir = root.path().join("state");
        let script = install_claude_session_start_script(&state_dir).expect("install script");
        assert_ne!(
            fs::metadata(&script)
                .expect("script metadata")
                .permissions()
                .mode()
                & 0o100,
            0,
            "owner execute bit"
        );

        let mut hook = std::process::Command::new(&script);
        hook.env("PIJ_RS_STATE_DIR", &state_dir)
            .env_remove("TMUX_PANE");
        let output = retry_etxtbsy(|| hook.output());

        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert_eq!(
            unstamped(&fs::read_to_string(state_dir.join("hook.log")).expect("hook log")),
            "skip:no-tmux\n"
        );
        assert_eq!(
            install_claude_session_start_script(&state_dir).expect("idempotent install"),
            script
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_script_adopt_emits_only_session_start_json() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new();
        let state_dir = root.path().join("state");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("bin directory");
        let path = bin_dir.join("pij-rs");
        fs::write(
            &path,
            "#!/bin/sh\ncase \"$1\" in ping|adopt) exit 0 ;; *) exit 2 ;; esac\n",
        )
        .expect("fake binary");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).expect("executable fake");
        let script = install_claude_session_start_script(&state_dir).expect("install script");

        let mut hook = std::process::Command::new(&script);
        hook.env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .env("PIJ_RS_STATE_DIR", &state_dir)
            .env("TMUX_PANE", "%42");
        let output = retry_etxtbsy(|| hook.output());

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("utf-8 stdout"),
            concat!(
                r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"pij automatically adopted this Claude session."}}"#,
                "\n"
            )
        );
        assert!(output.stderr.is_empty());
        assert!(!state_dir.join("hook.log").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refused_script_adopt_logs_one_line_and_stays_silent() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new();
        let state_dir = root.path().join("state");
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("bin directory");
        let path = bin_dir.join("pij-rs");
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$1\" = ping ]; then exit 0; fi\nprintf 'permission denied\\nsecond line\\n' >&2\nexit 2\n",
        )
        .expect("fake binary");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).expect("executable fake");
        let script = install_claude_session_start_script(&state_dir).expect("install script");

        let mut hook = std::process::Command::new(&script);
        hook.env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .env("PIJ_RS_STATE_DIR", &state_dir)
            .env("TMUX_PANE", "%42");
        let output = retry_etxtbsy(|| hook.output());

        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert_eq!(
            unstamped(&fs::read_to_string(state_dir.join("hook.log")).expect("hook log")),
            "refused:adopt pane=%42 reason=permission denied second line\n"
        );
    }

    const PROMPT_HOOK: &str = "/Users/test/.pij-rs/claude-user-prompt-submit-pij.sh";

    fn written_settings(home: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(home.join("settings.json")).expect("settings"))
            .expect("valid settings")
    }

    #[test]
    fn prompt_hook_joins_managed_session_start_without_disturbing_it() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/Users/test/.pij-rs/claude-session-start-pij.sh"}]}]},"theme":"dark"}"#;
        fs::write(home.join("settings.json"), original).expect("seed settings");

        let report = ensure_claude_user_prompt_submit_hook(
            std::slice::from_ref(&home),
            Path::new(PROMPT_HOOK),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(report.changed);
        assert_eq!(
            report.before, None,
            "SessionStart is not a stale prompt hook"
        );
        let written = fs::read_to_string(home.join("settings.json")).expect("settings");
        assert!(written.contains(
            r#""SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/Users/test/.pij-rs/claude-session-start-pij.sh"}]}]"#
        ));
        let parsed = written_settings(&home);
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(
            parsed["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            PROMPT_HOOK
        );
        // Each managed hook is settled once both are present; neither planner
        // claims the other's script.
        assert!(matches!(
            plan_claude_session_start_hook(
                Some(&written),
                "/Users/test/.pij-rs/claude-session-start-pij.sh"
            ),
            EnsurePlan::NoChange { .. }
        ));
        assert!(matches!(
            plan_claude_user_prompt_submit_hook(Some(&written), PROMPT_HOOK),
            EnsurePlan::NoChange { .. }
        ));
    }

    #[test]
    fn prompt_hook_reconciles_stale_managed_commands_and_keeps_foreign_ones() {
        let root = TestDir::new();
        let home = root.path().join(".claude");
        fs::create_dir_all(&home).expect("home");
        let original = r#"{"hooks":{"UserPromptSubmit":[{"matcher":"","hooks":[{"type":"command","command":"/tmp/a/claude-user-prompt-submit-pij.sh"}]},{"hooks":[{"type":"command","command":"foreign-prompt-guard"}]},{"matcher":"","hooks":[{"type":"command","command":"'/tmp/b c/claude-user-prompt-submit-pij.sh'"}]}]}}"#;
        fs::write(home.join("settings.json"), original).expect("seed settings");
        let before = inspect_claude_user_prompt_submit_hook(
            std::slice::from_ref(&home),
            Path::new(PROMPT_HOOK),
        )
        .into_iter()
        .next()
        .expect("inspection");
        assert!(!before.installed);
        assert_eq!(
            before.command.as_deref(),
            Some("/tmp/a/claude-user-prompt-submit-pij.sh")
        );

        let report = ensure_claude_user_prompt_submit_hook(
            std::slice::from_ref(&home),
            Path::new(PROMPT_HOOK),
        )
        .into_iter()
        .next()
        .expect("report");

        assert!(report.changed);
        assert_eq!(
            fs::read_to_string(report.backup.expect("backup")).expect("backup readable"),
            original
        );
        let parsed = written_settings(&home);
        let entries = parsed["hooks"]["UserPromptSubmit"]
            .as_array()
            .expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["hooks"][0]["command"], PROMPT_HOOK);
        assert_eq!(entries[1]["hooks"][0]["command"], "foreign-prompt-guard");
        let after = inspect_claude_user_prompt_submit_hook(
            std::slice::from_ref(&home),
            Path::new(PROMPT_HOOK),
        )
        .into_iter()
        .next()
        .expect("inspection");
        assert!(after.installed);
    }

    #[test]
    fn prompt_hook_covers_every_home_and_refuses_malformed_ones_untouched() {
        let root = TestDir::new();
        let claude = root.path().join(".claude");
        let alt = root.path().join(".claude-alt");
        fs::create_dir_all(&alt).expect("alt home");
        fs::write(alt.join("settings.json"), "{not-json").expect("seed alt");
        let fresh = root.path().join(".claude-fresh");

        let reports = ensure_claude_user_prompt_submit_hook(
            &[claude.clone(), alt.clone(), fresh.clone()],
            Path::new(PROMPT_HOOK),
        );

        assert_eq!(reports.len(), 3);
        assert!(reports[0].changed && reports[0].error.is_none());
        assert!(!reports[1].changed && reports[1].error.is_some());
        assert!(reports[2].changed && reports[2].error.is_none());
        for home in [&claude, &fresh] {
            assert_eq!(
                written_settings(home)["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
                PROMPT_HOOK
            );
        }
        assert_eq!(
            fs::read_to_string(alt.join("settings.json")).expect("alt settings"),
            "{not-json"
        );
        let again = ensure_claude_user_prompt_submit_hook(
            &[claude.clone(), fresh.clone()],
            Path::new(PROMPT_HOOK),
        );
        assert!(
            again
                .iter()
                .all(|report| !report.changed && report.backup.is_none())
        );
    }

    /// A fake `pij-rs` on PATH. `fyi-claim` behaves per `FAKE_MODE`, `activity`
    /// per `FAKE_ACTIVITY` (default: succeeds with stray stdout the hooks must
    /// swallow); every call is recorded as `<PIJ_SESSION_ID or unset>|<argv>`.
    #[cfg(unix)]
    struct HookRig {
        root: TestDir,
        script: PathBuf,
        path: String,
    }

    #[cfg(unix)]
    impl HookRig {
        fn prompt() -> Self {
            Self::new(install_claude_user_prompt_submit_script)
        }

        fn stop() -> Self {
            Self::new(install_claude_stop_script)
        }

        fn new(install: fn(&Path) -> std::io::Result<PathBuf>) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let root = TestDir::new();
            let bin_dir = root.path().join("bin");
            fs::create_dir_all(&bin_dir).expect("bin directory");
            let fake = bin_dir.join("pij-rs");
            fs::write(
                &fake,
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s|%s\\n' \"${PIJ_SESSION_ID:-unset}\" \"$*\" >>\"$FAKE_CALLS\"\n",
                    "if [ \"$1\" = activity ]; then\n",
                    "  case \"${FAKE_ACTIVITY:-ok}\" in\n",
                    "    fail) printf 'daemon down\\n' >&2; exit 4 ;;\n",
                    "    hang) exec sleep 30 ;;\n",
                    "  esac\n",
                    "  printf 'activity noise\\n'; exit 0\n",
                    "fi\n",
                    "case \"$FAKE_MODE\" in\n",
                    "  block) cat \"$FAKE_BLOCK\" ;;\n",
                    "  block-newline) cat \"$FAKE_BLOCK\"; printf '\\n' ;;\n",
                    "  empty) ;;\n",
                    "  fail) printf 'daemon unreachable\\n' >&2; exit 3 ;;\n",
                    "  hang) exec sleep 30 ;;\n",
                    "esac\n"
                ),
            )
            .expect("fake rs");
            let mut permissions = fs::metadata(&fake).expect("metadata").permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&fake, permissions).expect("executable fake");
            let jq_dir = std::env::var_os("PATH")
                .and_then(|path| std::env::split_paths(&path).find(|dir| dir.join("jq").is_file()))
                .map(|dir| format!(":{}", dir.display()))
                .unwrap_or_default();
            let path = format!("{}{jq_dir}:/usr/bin:/bin", bin_dir.display());
            let script = install(&root.path().join("state")).expect("install script");
            Self { root, script, path }
        }

        fn golden() -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../testkit/fixtures/golden/fyi/block.txt")
        }

        fn run(&self, mode: &str, payload: &str) -> (std::process::Output, std::time::Duration) {
            self.run_with(mode, "ok", payload)
        }

        fn run_with(
            &self,
            mode: &str,
            activity: &str,
            payload: &str,
        ) -> (std::process::Output, std::time::Duration) {
            use std::io::Write as _;
            use std::process::Stdio;

            let mut hook = std::process::Command::new(&self.script);
            hook.env("PATH", &self.path)
                .env("HOME", self.root.path())
                .env("PIJ_RS_STATE_DIR", self.root.path().join("state"))
                .env("PIJ_RS_FYI_CLAIM_TIMEOUT", "1")
                .env("PIJ_RS_ACTIVITY_TIMEOUT", "1")
                .env("TMUX_PANE", "%42")
                .env("PIJ_SESSION_ID", "pij-inherited-seat")
                .env("FAKE_MODE", mode)
                .env("FAKE_ACTIVITY", activity)
                .env("FAKE_BLOCK", Self::golden())
                .env("FAKE_CALLS", self.root.path().join("calls"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped());
            let started = std::time::Instant::now();
            let mut child = retry_etxtbsy(|| hook.spawn());
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(payload.as_bytes())
                .expect("write input");
            let output = wait_bounded(child, std::time::Duration::from_secs(10));
            (output, started.elapsed())
        }

        fn calls(&self) -> Vec<String> {
            fs::read_to_string(self.root.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn log(&self) -> String {
            fs::read_to_string(self.root.path().join("state/hook.log"))
                .map(|log| unstamped(&log))
                .unwrap_or_default()
        }
    }

    #[cfg(unix)]
    fn assert_prompt_block(output: std::process::Output, golden: &str, label: &str) {
        assert!(output.status.success(), "{label}");
        let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
        assert_eq!(stdout.lines().count(), 1, "one JSON document: {stdout}");
        let parsed: Value = serde_json::from_str(&stdout).expect("hook JSON");
        assert_eq!(
            parsed,
            serde_json::json!({"hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": golden,
            }}),
            "{label}"
        );
    }

    #[cfg(unix)]
    const WORKING_SESS_7: &str =
        "unset|activity --pane %42 --native-session sess-7 --state working";
    #[cfg(unix)]
    const CLAIM_SESS_7: &str =
        "unset|fyi-claim --pane %42 --native-session sess-7 --via hook:claude";

    #[cfg(unix)]
    #[test]
    fn prompt_hook_passes_the_claimed_block_through_byte_for_byte() {
        let rig = HookRig::prompt();
        let golden = fs::read_to_string(HookRig::golden()).expect("golden block");
        for mode in ["block", "block-newline"] {
            let (output, _) = rig.run(
                mode,
                r#"{"session_id":"sess-7","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
            );
            assert_prompt_block(output, &golden, mode);
        }
        assert_eq!(
            rig.calls(),
            [WORKING_SESS_7, CLAIM_SESS_7, WORKING_SESS_7, CLAIM_SESS_7],
            "each prompt publishes working, then claims once; both bound by pane and session, never an inherited seat"
        );
        assert_eq!(rig.log(), "", "a healthy prompt logs nothing");
    }

    #[cfg(unix)]
    #[test]
    fn prompt_hook_still_claims_when_publishing_working_fails() {
        let rig = HookRig::prompt();
        let golden = fs::read_to_string(HookRig::golden()).expect("golden block");
        let (output, _) = rig.run_with("block", "fail", r#"{"session_id":"sess-7"}"#);

        assert_prompt_block(output, &golden, "activity refused");
        assert_eq!(rig.calls(), [WORKING_SESS_7, CLAIM_SESS_7]);
        assert_eq!(
            rig.log(),
            "refused:activity pane=%42 state=working status=4 reason=daemon down \n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn prompt_hook_is_silent_when_nothing_is_held_or_the_claim_fails() {
        let rig = HookRig::prompt();
        for mode in ["empty", "fail"] {
            let (output, _) = rig.run(mode, "{}");
            assert!(output.status.success(), "{mode}");
            assert!(output.stdout.is_empty(), "{mode} printed output");
        }
        assert_eq!(
            rig.calls(),
            [
                "unset|activity --pane %42 --state working",
                "unset|fyi-claim --pane %42 --via hook:claude",
            ]
            .repeat(2),
            "no session in the payload: the pane alone binds both calls"
        );
        let log = fs::read_to_string(rig.root.path().join("state/hook.log")).expect("hook log");
        assert_eq!(
            unstamped(&log),
            "refused:fyi-claim pane=%42 status=3 reason=daemon unreachable \n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn prompt_hook_gives_up_on_a_wedged_daemon_without_holding_the_prompt() {
        let rig = HookRig::prompt();
        let (output, took) = rig.run_with("hang", "hang", r#"{"session_id":"sess-7"}"#);

        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            took < std::time::Duration::from_secs(5),
            "prompt held {took:?}"
        );
        assert_eq!(rig.calls(), [WORKING_SESS_7, CLAIM_SESS_7]);
    }

    const STOP_HOOK: &str = "/Users/test/.pij-rs/claude-stop-pij.sh";

    /// Review N1: a turn that ends on an API error fires StopFailure, not Stop,
    /// so the same idle script is registered there too, in every home, beside
    /// (not instead of) the Stop hook.
    #[test]
    fn stop_failure_runs_the_idle_script_in_every_home_beside_stop() {
        let root = TestDir::new();
        let claude = root.path().join(".claude");
        let alt = root.path().join(".claude-alt");
        fs::create_dir_all(&claude).expect("home");
        let homes = [claude.clone(), alt.clone()];
        assert!(
            ensure_claude_stop_hook(&homes, Path::new(STOP_HOOK))
                .iter()
                .all(|report| report.error.is_none())
        );
        let reports = ensure_claude_stop_failure_hook(&homes, Path::new(STOP_HOOK));
        assert!(
            reports
                .iter()
                .all(|report| report.changed && report.error.is_none())
        );
        for home in [&claude, &alt] {
            let parsed = written_settings(home);
            assert_eq!(
                parsed["hooks"]["StopFailure"][0]["hooks"][0]["command"],
                STOP_HOOK
            );
            assert_eq!(parsed["hooks"]["Stop"][0]["hooks"][0]["command"], STOP_HOOK);
        }
        assert!(
            inspect_claude_stop_failure_hook(&homes, Path::new(STOP_HOOK))
                .iter()
                .all(|state| state.installed)
        );
        assert!(
            ensure_claude_stop_failure_hook(&homes, Path::new(STOP_HOOK))
                .iter()
                .all(|report| !report.changed),
            "idempotent"
        );
    }

    #[test]
    fn stop_hook_covers_every_home_reconciling_stale_and_keeping_foreign() {
        let root = TestDir::new();
        let claude = root.path().join(".claude");
        let alt = root.path().join(".claude-alt");
        fs::create_dir_all(&claude).expect("home");
        let original = r#"{"hooks":{"UserPromptSubmit":[{"matcher":"","hooks":[{"type":"command","command":"/Users/test/.pij-rs/claude-user-prompt-submit-pij.sh"}]}],"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/tmp/a/claude-stop-pij.sh"}]},{"hooks":[{"type":"command","command":"foreign-stop-notifier"}]},{"matcher":"","hooks":[{"type":"command","command":"'/tmp/b c/claude-stop-pij.sh'"}]}]}}"#;
        fs::write(claude.join("settings.json"), original).expect("seed settings");
        let homes = [claude.clone(), alt.clone()];
        let before = inspect_claude_stop_hook(&homes, Path::new(STOP_HOOK));
        assert!(!before[0].installed);
        assert_eq!(
            before[0].command.as_deref(),
            Some("/tmp/a/claude-stop-pij.sh")
        );
        assert!(!before[1].file_exists && !before[1].installed);

        let reports = ensure_claude_stop_hook(&homes, Path::new(STOP_HOOK));

        assert!(
            reports
                .iter()
                .all(|report| report.changed && report.error.is_none())
        );
        assert_eq!(
            fs::read_to_string(reports[0].backup.as_ref().expect("backup")).expect("backup"),
            original
        );
        let parsed = written_settings(&claude);
        let entries = parsed["hooks"]["Stop"].as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["hooks"][0]["command"], STOP_HOOK);
        assert_eq!(entries[1]["hooks"][0]["command"], "foreign-stop-notifier");
        assert_eq!(
            parsed["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], PROMPT_HOOK,
            "the prompt hook is not the Stop hook's to reconcile"
        );
        assert_eq!(
            written_settings(&alt)["hooks"]["Stop"][0]["hooks"][0]["command"],
            STOP_HOOK
        );
        assert!(
            inspect_claude_stop_hook(&homes, Path::new(STOP_HOOK))
                .iter()
                .all(|state| state.installed)
        );
        assert!(
            ensure_claude_stop_hook(&homes, Path::new(STOP_HOOK))
                .iter()
                .all(|report| !report.changed && report.backup.is_none())
        );
    }

    #[cfg(unix)]
    #[test]
    fn stop_hook_publishes_idle_and_prints_nothing() {
        let rig = HookRig::stop();
        let (output, _) = rig.run(
            "block",
            r#"{"session_id":"sess-7","hook_event_name":"Stop","stop_hook_active":false}"#,
        );
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "Stop-hook stdout can block a stop"
        );

        let (output, _) = rig.run("block", "{}");
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            rig.calls(),
            [
                "unset|activity --pane %42 --native-session sess-7 --state idle",
                "unset|activity --pane %42 --state idle",
            ],
            "idle only, bound by pane and session, never an inherited seat"
        );
        assert_eq!(rig.log(), "");
    }

    #[cfg(unix)]
    #[test]
    fn stop_hook_is_silent_and_bounded_when_the_daemon_fails_or_hangs() {
        let rig = HookRig::stop();
        for activity in ["fail", "hang"] {
            let (output, took) = rig.run_with("block", activity, r#"{"session_id":"sess-7"}"#);
            assert!(output.status.success(), "{activity}");
            assert!(output.stdout.is_empty(), "{activity} printed output");
            assert!(
                took < std::time::Duration::from_secs(3),
                "{activity} held the stop {took:?}"
            );
        }
        assert_eq!(
            rig.log(),
            concat!(
                "refused:activity pane=%42 state=idle status=4 reason=daemon down \n",
                "refused:activity pane=%42 state=idle status=143 reason=\n",
            ),
            "one line per failure"
        );
    }
}
