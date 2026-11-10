//! The pij-managed Claude hooks as one set (plan 226): `pij-rs doctor
//! claude-hooks` reports each one missing or stale with its fix, and
//! `pij-rs claude-hooks --remove` takes them all out for `just unlink`.

use std::path::{Path, PathBuf};

use pij_harnesses::{
    ClaudeHookState, EnsureReport, claude_script_is_current, inspect_claude_message_display_hook,
    inspect_claude_session_start_hook, inspect_claude_stop_failure_hook, inspect_claude_stop_hook,
    inspect_claude_user_prompt_submit_hook, remove_claude_hooks,
};

/// The daemon installs every managed script and hook when it starts.
pub const FIX: &str = "just bounce-rs";

type Inspect = fn(&[PathBuf], &Path) -> Vec<ClaudeHookState>;

/// Every managed hook: its Claude event, its script, and how to inspect it.
const HOOKS: [(&str, &str, Inspect); 5] = [
    (
        "SessionStart",
        "claude-session-start-pij.sh",
        inspect_claude_session_start_hook,
    ),
    (
        "UserPromptSubmit",
        "claude-user-prompt-submit-pij.sh",
        inspect_claude_user_prompt_submit_hook,
    ),
    ("Stop", "claude-stop-pij.sh", inspect_claude_stop_hook),
    (
        "StopFailure",
        "claude-stop-pij.sh",
        inspect_claude_stop_failure_hook,
    ),
    (
        "MessageDisplay",
        "claude-message-display-pij.sh",
        inspect_claude_message_display_hook,
    ),
];

/// One managed hook in one Claude home.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HookRow {
    /// The Claude hook event.
    pub hook: &'static str,
    /// The Claude configuration home.
    pub home: PathBuf,
    /// `installed`, `script-missing`, `script-stale`, `wrong-command`,
    /// `hook-missing`, `file-missing` or `error`.
    pub status: &'static str,
    /// The managed command found there, if any.
    pub configured: Option<String>,
    /// The script the daemon installs.
    pub expected: PathBuf,
    /// What to run when the status is not `installed`.
    pub fix: Option<String>,
    /// Why the settings could not be read or parsed.
    pub error: Option<String>,
}

/// Inspect every managed hook in every home, read-only.
#[must_use]
pub fn doctor_rows(homes: &[PathBuf], state_dir: &Path) -> Vec<HookRow> {
    let mut rows = Vec::with_capacity(HOOKS.len() * homes.len());
    for (hook, script, inspect) in HOOKS {
        let expected = state_dir.join(script);
        let current = claude_script_is_current(state_dir, script);
        for state in inspect(homes, &expected) {
            let status = if state.error.is_some() {
                "error"
            } else if state.installed && !expected.is_file() {
                "script-missing"
            } else if state.installed && current == Some(false) {
                "script-stale"
            } else if state.installed {
                "installed"
            } else if state.command.is_some() {
                "wrong-command"
            } else if state.file_exists {
                "hook-missing"
            } else {
                "file-missing"
            };
            let fix = match status {
                "installed" => None,
                "error" => Some(format!(
                    "repair {}, then {FIX}",
                    state.home.join("settings.json").display()
                )),
                _ => Some(FIX.to_string()),
            };
            rows.push(HookRow {
                hook,
                home: state.home,
                status,
                configured: state.command,
                expected: expected.clone(),
                fix,
                error: state.error,
            });
        }
    }
    rows
}

/// One line when all is well; otherwise one line per problem, with its fix.
#[must_use]
pub fn render_doctor(rows: &[HookRow]) -> String {
    let problems = rows
        .iter()
        .filter(|row| row.status != "installed")
        .collect::<Vec<_>>();
    if problems.is_empty() {
        let homes = rows
            .iter()
            .map(|row| &row.home)
            .collect::<std::collections::BTreeSet<_>>();
        return format!(
            "claude hooks: ok ({} in {} home{})",
            HOOKS.map(|(hook, ..)| hook).join(", "),
            homes.len(),
            if homes.len() == 1 { "" } else { "s" }
        );
    }
    let mut out = format!(
        "claude hooks: {} problem{}",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" }
    );
    for row in problems {
        out.push_str(&format!(
            "\n  {} in {}: {}{} (fix: {})",
            row.hook,
            row.home.display(),
            row.status,
            row.error
                .as_deref()
                .map(|error| format!(", {error}"))
                .unwrap_or_default(),
            row.fix.as_deref().unwrap_or(FIX)
        ));
    }
    out
}

/// One home's removal.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RemoveRow {
    /// The Claude configuration home.
    pub home: PathBuf,
    /// Whether settings.json was rewritten.
    pub changed: bool,
    /// The managed commands taken out, by event.
    pub removed: Option<String>,
    /// The copy of settings.json made before the rewrite.
    pub backup: Option<PathBuf>,
    /// Why this home was left untouched.
    pub error: Option<String>,
}

/// Remove every managed hook from every home; other settings stay.
#[must_use]
pub fn remove(homes: &[PathBuf]) -> Vec<RemoveRow> {
    remove_claude_hooks(homes)
        .into_iter()
        .map(|report: EnsureReport| RemoveRow {
            home: report.home,
            changed: report.changed,
            removed: report.changed.then_some(report.before).flatten(),
            backup: report.backup,
            error: report.error,
        })
        .collect()
}

/// One line per home.
#[must_use]
pub fn render_remove(rows: &[RemoveRow]) -> String {
    rows.iter()
        .map(|row| match (&row.error, row.changed) {
            (Some(error), _) => format!(
                "claude hooks: left {} untouched: {error}",
                row.home.display()
            ),
            (None, true) => format!(
                "claude hooks: removed from {} (backup {})",
                row.home.display(),
                row.backup
                    .as_deref()
                    .map_or_else(|| "none".to_string(), |path| path.display().to_string())
            ),
            (None, false) => format!("claude hooks: none in {}", row.home.display()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_harnesses::{
        ensure_claude_message_display_hook, ensure_claude_stop_hook,
        install_claude_message_display_script,
    };

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pij-claude-hooks-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn status(rows: &[HookRow], hook: &str) -> &'static str {
        rows.iter()
            .find(|row| row.hook == hook)
            .expect("row")
            .status
    }

    #[test]
    fn doctor_reports_missing_and_stale_with_the_fix() {
        let root = scratch("doctor");
        let home = root.join(".claude");
        let state = root.join("state");
        let homes = [home.clone()];

        let rows = doctor_rows(&homes, &state);
        assert!(rows.iter().all(|row| row.status == "file-missing"));
        assert!(render_doctor(&rows).contains("MessageDisplay in"));

        let script = install_claude_message_display_script(&state).expect("install");
        let _ = ensure_claude_message_display_hook(&homes, &script);
        let _ = ensure_claude_stop_hook(&homes, &state.join("claude-stop-pij.sh"));
        let rows = doctor_rows(&homes, &state);
        assert_eq!(status(&rows, "MessageDisplay"), "installed");
        assert_eq!(status(&rows, "Stop"), "script-missing");
        assert_eq!(status(&rows, "SessionStart"), "hook-missing");

        std::fs::write(&script, "#!/bin/sh\n# an older build\n").expect("stale script");
        let rows = doctor_rows(&homes, &state);
        let display = rows
            .iter()
            .find(|row| row.hook == "MessageDisplay")
            .expect("row");
        assert_eq!(display.status, "script-stale");
        assert_eq!(display.fix.as_deref(), Some(FIX));
        let rendered = render_doctor(&rows);
        assert!(
            rendered.contains(&format!(
                "MessageDisplay in {}: script-stale (fix: just bounce-rs)",
                home.display()
            )),
            "{rendered}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removal_reports_each_home() {
        let root = scratch("remove");
        let home = root.join(".claude");
        let script = root.join("state/claude-message-display-pij.sh");
        let _ = ensure_claude_message_display_hook(std::slice::from_ref(&home), &script);

        let rows = remove(&[home.clone(), root.join(".claude-none")]);
        assert!(rows[0].changed && rows[0].backup.is_some());
        assert!(
            rows[0]
                .removed
                .as_deref()
                .is_some_and(|removed| removed.starts_with("MessageDisplay: "))
        );
        assert!(!rows[1].changed && rows[1].error.is_none());
        let rendered = render_remove(&rows);
        assert!(rendered.contains("removed from"), "{rendered}");
        assert!(rendered.contains("none in"), "{rendered}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
