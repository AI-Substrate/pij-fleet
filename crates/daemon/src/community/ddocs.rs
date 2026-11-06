//! ddocs: found once at daemon start (ruling C10), launched by absolute path.
//!
//! `PIJ_DDOCS` wins when set; otherwise the first `ddocs` on PATH. The writer
//! talks to ddocs through [`Validator`], injected through its constructor, so
//! unit tests run without ddocs installed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use pij_core::program::Resolver;
use serde_json::Value;

use super::{E_DDOCS_INVALID, E_DDOCS_MISSING, Refusal};

/// What to do when ddocs is missing.
pub const INSTALL_STEP: &str = "Install ddocs (the @ai-substrate/dd package) so `ddocs` is on the daemon's PATH, or set PIJ_DDOCS to its absolute path; then restart the daemon (`just bounce-rs`).";

/// Validate and render one `.dd.json` document. Paths are absolute.
pub trait Validator: Send + Sync {
    /// `ddocs validate`: `Ok` when the document is clean.
    ///
    /// # Errors
    /// [`E_DDOCS_INVALID`] carrying ddocs' own message, or [`E_DDOCS_MISSING`].
    fn validate(&self, worktree: &Path, document: &Path) -> Result<(), Refusal>;

    /// `ddocs build`: write the `.dd.md` sibling.
    ///
    /// # Errors
    /// As [`Self::validate`].
    fn build(&self, worktree: &Path, document: &Path) -> Result<(), Refusal>;
}

/// The ddocs this daemon will use, decided once at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ddocs {
    /// An executable at an absolute path.
    Found(PathBuf),
    /// Why there is none.
    Missing(String),
}

impl Ddocs {
    /// Resolve from the process environment. Call once, at daemon start.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var_os("PIJ_DDOCS").as_deref(),
            std::env::var_os("PATH").as_deref(),
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        )
    }

    /// Resolve from explicit `PIJ_DDOCS` and `PATH` values, as seen from `cwd`.
    ///
    /// A relative or empty PATH entry names a directory relative to `cwd`, the
    /// daemon's directory at start. Every entry is made absolute before the
    /// search, so the path kept is absolute and still names the same program
    /// after ddocs is launched in the governance worktree (review of #114, F002).
    pub fn resolve(pij_ddocs: Option<&OsStr>, path: Option<&OsStr>, cwd: &Path) -> Self {
        match resolve_tool("ddocs", "PIJ_DDOCS", pij_ddocs, path, cwd) {
            Ok(found) => Self::Found(found),
            Err(reason) => Self::Missing(reason),
        }
    }

    /// The resolved path, for ping and `just doctor`.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            Self::Missing(_) => None,
        }
    }

    fn run(&self, worktree: &Path, args: &[&OsStr]) -> Result<(), Refusal> {
        let program = match self {
            Self::Found(path) => path,
            Self::Missing(reason) => {
                return Err(Refusal::new(E_DDOCS_MISSING, reason.clone()).with_next(INSTALL_STEP));
            }
        };
        pij_core::spawns::record("ddocs.community");
        let output = pij_core::program::launch(program, |ddocs| {
            Command::new(ddocs)
                .current_dir(worktree)
                .arg("--json")
                .args(args)
                .output()
        })
        .map_err(|error| {
            Refusal::new(
                E_DDOCS_MISSING,
                format!("could not run {}: {error}", program.display()),
            )
            .with_next(INSTALL_STEP)
        })?;
        if output.status.success() {
            return Ok(());
        }
        Err(Refusal::new(
            E_DDOCS_INVALID,
            ddocs_message(&output.stdout, &output.stderr),
        ))
    }
}

impl Validator for Ddocs {
    fn validate(&self, worktree: &Path, document: &Path) -> Result<(), Refusal> {
        self.run(
            worktree,
            &[
                OsStr::new("validate"),
                document.as_os_str(),
                OsStr::new("--depth"),
                OsStr::new("0"),
            ],
        )
    }

    fn build(&self, worktree: &Path, document: &Path) -> Result<(), Refusal> {
        self.run(worktree, &[OsStr::new("build"), document.as_os_str()])
    }
}

/// The `pij directory add` flag that fills a pij/role location ddocs found
/// missing (plan 209, F36).
fn role_flag(location: &str) -> Option<&'static str> {
    const FLAGS: [(&str, &str); 7] = [
        ("[meta].value.title", "--title <text>"),
        ("[meta].value.kind", "--kind specialist|general"),
        ("[meta].value.lifecycle", "--lifecycle long-lived|per-task"),
        ("[meta].value.holders", "--holders one|many"),
        ("sections[scope]", "--rule <text>"),
        (
            "sections[standing_packet]",
            "--packet <text> or --brief <path>",
        ),
        ("sections[default_seat]", "--harness <h>"),
    ];
    FLAGS
        .iter()
        .find(|(at, _)| location.contains(at))
        .map(|(_, flag)| *flag)
}

/// ddocs' own words: its error message, each issue's location and message, and
/// its next action. Falls back to raw output when it printed no envelope. For
/// a role, the next step names the flags to add, never a hand edit (F36).
fn ddocs_message(stdout: &[u8], stderr: &[u8]) -> String {
    let Ok(envelope) = serde_json::from_slice::<Value>(stdout) else {
        let raw = String::from_utf8_lossy(if stdout.is_empty() { stderr } else { stdout });
        return format!("ddocs failed: {}", raw.trim());
    };
    let error = &envelope["error"];
    let mut message = error["message"]
        .as_str()
        .unwrap_or("ddocs refused the document")
        .to_string();
    let role = error["details"]["schema"].as_str() == Some("pij/role");
    let mut flags: Vec<&str> = Vec::new();
    for issue in error["details"]["issues"].as_array().into_iter().flatten() {
        let location = issue["location"].as_str().unwrap_or("?");
        let text = issue["message"].as_str().unwrap_or("");
        message.push_str(&format!("\n  {location}: {text}"));
        if let Some(flag) = role.then(|| role_flag(location)).flatten()
            && !flags.contains(&flag)
        {
            flags.push(flag);
        }
    }
    if !flags.is_empty() {
        message.push_str(&format!(
            "\nNothing was written. Add {} and run the add again.",
            flags.join(", ")
        ));
    } else if let Some(next) = envelope["next_action"].as_str() {
        message.push_str(&format!("\n{next}"));
    }
    message
}

/// Find a tool once, at daemon start: `$<env>` when set (absolute and
/// executable), else the first `name` on PATH, every PATH entry made absolute
/// against `cwd` so the kept path names the same program from any cwd.
/// Shared by ddocs (C10) and gh (plan 196).
///
/// # Errors
/// Why there is none, naming the variable.
pub(crate) fn resolve_tool(
    name: &str,
    env: &str,
    chosen: Option<&OsStr>,
    path: Option<&OsStr>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    if let Some(chosen) = chosen.filter(|value| !value.is_empty()) {
        let chosen = Path::new(chosen);
        if !chosen.is_absolute() {
            return Err(format!(
                "{env}={} is not an absolute path",
                chosen.display()
            ));
        }
        // A path holding `/` comes back unchanged; check it as posix_spawn would.
        return if executable(chosen) {
            Ok(chosen.to_path_buf())
        } else {
            Err(format!(
                "{env}={} is not an executable file",
                chosen.display()
            ))
        };
    }
    let absolute = path.and_then(|path| {
        std::env::join_paths(
            std::env::split_paths(path).map(|dir| cwd.join(dir).components().collect::<PathBuf>()),
        )
        .ok()
    });
    let found = Resolver::new().resolve(OsStr::new(name), absolute.as_deref().or(path));
    if Path::new(&found).is_absolute() {
        Ok(PathBuf::from(found))
    } else {
        Err(format!(
            "{name} is not on the daemon's PATH and {env} is unset"
        ))
    }
}

/// The test `posix_spawn` applies (a regular file `access(X_OK)` allows),
/// asked of the resolver by searching only the file's own directory.
fn executable(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    Resolver::new().resolve(name, Some(dir.as_os_str())) == path.as_os_str()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// ddocs' refusal of a role missing kind, scope and its standing packet.
    fn three_gaps() -> String {
        let envelope = serde_json::json!({
            "error": {
                "message": "missing required field \"kind\"",
                "details": {"schema": "pij/role", "issues": [
                    {"location": "$.sections[meta].value.kind", "message": "missing required field \"kind\""},
                    {"location": "$.sections[scope]", "message": "missing required section \"scope\""},
                    {"location": "$.sections[standing_packet]", "message": "missing required section \"standing_packet\""},
                ]},
            },
            "next_action": "Fix role.dd.json at $.sections[meta].value.kind, then re-run `ddocs validate`.",
        });
        ddocs_message(envelope.to_string().as_bytes(), b"")
    }

    /// Plan 209, F36: the next step names the flags to add, never a hand edit.
    #[test]
    fn a_role_refusal_names_the_flag_for_each_gap_not_a_hand_edit() {
        let message = three_gaps();
        assert!(
            message.ends_with(
                "Nothing was written. Add --kind specialist|general, --rule <text>, \
                 --packet <text> or --brief <path> and run the add again."
            ),
            "{message}"
        );
        assert!(!message.contains("Fix role.dd.json"), "{message}");
    }

    /// Plan 209, F37: one refusal lists every missing section.
    #[test]
    fn a_role_refusal_lists_every_gap_at_once() {
        let message = three_gaps();
        for gap in [
            "$.sections[meta].value.kind: missing",
            "$.sections[scope]: missing",
            "$.sections[standing_packet]: missing",
        ] {
            assert!(message.contains(gap), "{gap}: {message}");
        }
    }

    fn tool(dir: &Path, name: &str, mode: u32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn pij_ddocs_wins_over_path_and_must_be_an_absolute_executable() {
        let root = pij_testkit::fresh_dir("pij-ddocs-resolve");
        let on_path = tool(&root.join("bin"), "ddocs", 0o755);
        let chosen = tool(&root.join("pinned"), "ddocs-pinned", 0o755);
        let path = std::env::join_paths([root.join("bin")]).unwrap();
        assert_eq!(
            Ddocs::resolve(Some(chosen.as_os_str()), Some(&path), &root),
            Ddocs::Found(chosen.clone())
        );
        assert_eq!(
            Ddocs::resolve(None, Some(&path), &root),
            Ddocs::Found(on_path.clone())
        );
        assert_eq!(
            Ddocs::resolve(Some(OsStr::new("")), Some(&path), &root),
            Ddocs::Found(on_path)
        );

        let flat = tool(&root.join("pinned"), "not-exec", 0o644);
        let Ddocs::Missing(reason) = Ddocs::resolve(Some(flat.as_os_str()), Some(&path), &root)
        else {
            panic!("a non-executable PIJ_DDOCS is missing, not found");
        };
        assert!(reason.contains("not an executable file"), "{reason}");
        let Ddocs::Missing(reason) = Ddocs::resolve(Some(OsStr::new("ddocs")), Some(&path), &root)
        else {
            panic!("a relative PIJ_DDOCS is missing");
        };
        assert!(reason.contains("not an absolute path"), "{reason}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Review of #114, F002: a ddocs found through a relative or empty PATH
    /// entry is kept as an absolute path, so it still launches when the
    /// writer runs it with the governance worktree as its cwd.
    #[test]
    fn a_relative_or_empty_path_entry_resolves_absolutely_and_launches_from_another_cwd() {
        let root = pij_testkit::fresh_dir("pij-ddocs-relative")
            .canonicalize()
            .unwrap();
        let bin = root.join("tools");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("ddocs");
        std::fs::write(&script, "#!/bin/sh\necho '{\"status\":\"ok\"}'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let worktree = root.join("governance");
        std::fs::create_dir_all(&worktree).unwrap();
        let document = worktree.join("role.dd.json");

        let absolute = Ddocs::resolve(Some(script.as_os_str()), None, &root);
        absolute
            .validate(&worktree, &document)
            .expect("control: the absolute path launches from the governance cwd");
        for (path, cwd) in [
            ("tools", root.as_path()),
            ("./tools", root.as_path()),
            ("", bin.as_path()),
            ("/nonexistent::/also-not", bin.as_path()),
        ] {
            let ddocs = Ddocs::resolve(None, Some(OsStr::new(path)), cwd);
            assert_eq!(
                ddocs.path(),
                Some(script.as_path()),
                "PATH={path:?} from {cwd:?}"
            );
            ddocs
                .validate(&worktree, &document)
                .unwrap_or_else(|refusal| panic!("PATH={path:?}: {refusal}"));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_ddocs_refuses_by_name_with_the_install_step() {
        let empty = pij_testkit::fresh_dir("pij-ddocs-missing");
        let path = std::env::join_paths([&empty]).unwrap();
        let ddocs = Ddocs::resolve(None, Some(&path), &empty);
        assert_eq!(ddocs.path(), None);
        let refusal = ddocs
            .validate(&empty, &empty.join("x.dd.json"))
            .expect_err("no ddocs");
        assert_eq!(refusal.code, E_DDOCS_MISSING);
        assert_eq!(refusal.next.as_deref(), Some(INSTALL_STEP));
        let _ = std::fs::remove_dir_all(empty);
    }

    #[test]
    fn an_invalid_document_refuses_with_ddocs_own_words() {
        let stdout = br#"{"status":"error","error":{"code":"E402","message":"missing required field \"scope\"","details":{"issues":[{"location":"$.sections[meta].value.scope","message":"missing required field \"scope\""}]}},"next_action":"Fix it."}"#;
        let message = ddocs_message(stdout, b"");
        assert_eq!(
            message,
            "missing required field \"scope\"\n  $.sections[meta].value.scope: missing required field \"scope\"\nFix it."
        );
        assert_eq!(ddocs_message(b"", b"boom\n"), "ddocs failed: boom");
    }

    /// The one test that runs the real ddocs. It names its skip when absent.
    #[test]
    fn real_ddocs_validates_and_builds_or_names_its_skip() {
        let Ddocs::Found(path) = Ddocs::from_env() else {
            eprintln!("SKIP real_ddocs_validates_and_builds: ddocs is not installed here");
            return;
        };
        let ddocs = Ddocs::Found(path);
        let root = pij_testkit::fresh_dir("pij-ddocs-real")
            .canonicalize()
            .unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let schema = root.join(".dd/schemas/x/thing");
        std::fs::create_dir_all(&schema).unwrap();
        std::fs::write(
            schema.join("schema.json"),
            r#"{"dd_schema":1,"name":"x/thing","description":"t","sections":{"meta":{"shape":{"type":"object","required":["id"],"fields":{"id":{"type":"string"}}}}}}"#,
        )
        .unwrap();
        let good = root.join("good.dd.json");
        let bad = root.join("bad.dd.json");
        std::fs::write(&good, r#"{"dd":{"schema":"x/thing"},"sections":[{"name":"meta","value":{"id":"a"}}],"references":[]}"#).unwrap();
        std::fs::write(&bad, r#"{"dd":{"schema":"x/thing"},"sections":[{"name":"meta","value":{}}],"references":[]}"#).unwrap();
        ddocs
            .validate(&root, &good)
            .expect("a clean document validates");
        ddocs.build(&root, &good).expect("and builds");
        assert!(root.join("good.dd.md").is_file());
        let refusal = ddocs
            .validate(&root, &bad)
            .expect_err("a bad document refuses");
        assert_eq!(refusal.code, E_DDOCS_INVALID);
        assert!(
            refusal.message.contains("missing required field \"id\""),
            "{refusal}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
