//! Golden byte parity with the plan-156 bash script (plan 180).
//!
//! Each case runs the PRESERVED script (tests/fixtures/copilot-statusline-plan156.sh)
//! with real bash, jq, awk and git, and stub `pij-rs` / `pij` that print canned
//! whoami text. The Rust renderer gets the same inputs. The bytes must match.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Context, Whoami, git_branch, render_copilot};

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/copilot-statusline-plan156.sh"
);
static COUNTER: AtomicU64 = AtomicU64::new(0);

const OK: &str = r#"{"ok":true,"command":"pij whoami","data":{"id":"pij-brave-otter","harness":"copilot","pending_fyis":0}}"#;
const OK_FYI: &str =
    r#"{"ok":true,"command":"pij whoami","data":{"id":"pij-brave-otter","pending_fyis":3}}"#;
const DEAD: &str = r#"{"ok":false,"command":"pij whoami","error":{"kind":"refused","message":"pane %9 belongs to seat `pij-gone-heron`, whose process is gone"}}"#;
const UNREGISTERED: &str = r#"{"ok":false,"command":"pij whoami","error":{"kind":"refused","message":"no seat is registered for pane %9"}}"#;
const MALFORMED_OK: &str = r#"{"ok":true, broken"#;

fn have(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .is_ok_and(|output| output.status.success())
}

struct World {
    root: PathBuf,
    bin: PathBuf,
    home: PathBuf,
    pwd: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
    detached: PathBuf,
    ambiguous: PathBuf,
    plain: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl World {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pij-statusline-{}-{n}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
                % 1_000_000
        ));
        let [bin, home, pwd, repo, detached, ambiguous, plain] = [
            "bin",
            "home",
            "pwd",
            "repo",
            "detached",
            "ambiguous",
            "plain dir ✓",
        ]
        .map(|name| root.join(name));
        for dir in [&bin, &home, &pwd, &repo, &detached, &ambiguous, &plain] {
            fs::create_dir_all(dir).expect("dir");
        }
        // Stubs print canned whoami text, like `pij-rs --json whoami` would.
        for (name, file) in [("pij-rs", "whoami.txt"), ("pij", "pij.txt")] {
            let stub = bin.join(name);
            fs::write(
                &stub,
                format!(
                    "#!/bin/sh\ncat '{}' 2>/dev/null\n",
                    root.join(file).display()
                ),
            )
            .expect("stub");
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        git(&repo, &["init", "-q", "-b", "feat/spawn-storm"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        fs::create_dir_all(repo.join("sub/dir")).expect("subdir");
        let worktree = root.join("linked");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt-branch",
                worktree.to_str().expect("utf8"),
            ],
        );
        git(&detached, &["init", "-q", "-b", "main"]);
        git(&detached, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&detached, &["checkout", "-q", "--detach"]);
        // A tag sharing the branch's name: git prints `heads/dup`.
        git(&ambiguous, &["init", "-q", "-b", "dup"]);
        git(&ambiguous, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&ambiguous, &["tag", "dup"]);
        Self {
            root,
            bin,
            home,
            pwd,
            repo,
            worktree,
            detached,
            ambiguous,
            plain,
        }
    }

    fn write(&self, path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        fs::write(path, text).expect("write");
    }

    /// The plan-156 script's exact stdout.
    fn script(
        &self,
        payload: &str,
        pane: Option<&str>,
        whoami: Option<&str>,
        pij: Option<&str>,
    ) -> Vec<u8> {
        let _ = fs::remove_file(self.root.join("whoami.txt"));
        let _ = fs::remove_file(self.root.join("pij.txt"));
        if let Some(text) = whoami {
            self.write(&self.root.join("whoami.txt"), text);
        }
        if let Some(text) = pij {
            self.write(&self.root.join("pij.txt"), text);
        }
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("/bin/bash");
        command
            .arg(SCRIPT)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("LC_ALL", "C.UTF-8")
            .env("PIJ_SESSION_ID", "pij-inherited-must-not-show")
            .current_dir(&self.pwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(pane) = pane {
            command.env("TMUX_PANE", pane);
        }
        let mut child = command.spawn().expect("bash");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("payload");
        child.wait_with_output().expect("script").stdout
    }

    fn rust(
        &self,
        payload: &str,
        pane: Option<&str>,
        whoami: Option<&str>,
        pij: Option<&str>,
    ) -> String {
        let whoami = match pane.filter(|pane| !pane.is_empty()) {
            Some(_) => Whoami::Pane(whoami.unwrap_or_default().to_string()),
            None => Whoami::Plain(pij.and_then(|text| {
                let value: serde_json::Value = serde_json::from_str(text).ok()?;
                value["data"]["id"]
                    .as_str()
                    .or_else(|| value["id"].as_str())
                    .map(str::to_string)
            })),
        };
        let context = Context {
            pwd: self.pwd.clone(),
            home: Some(self.home.clone()),
            whoami,
        };
        render_copilot(payload.as_bytes(), &context, &git_branch)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree)
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn escape(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\x1b', "\\e")
}

fn payload(cwd: &Path, extra: &str) -> String {
    format!(
        r#"{{"workspace":{{"current_dir":{}}},"model":{{"display_name":"Claude Opus 5.5","id":"claude-opus-5.5"}}{extra}}}"#,
        serde_json::to_string(cwd.to_str().expect("utf8")).expect("json")
    )
}

/// (label, payload, TMUX_PANE, whoami text, pij text).
type Case = (
    String,
    String,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// An effort-settings arrangement applied before a pass over the corpus.
type EffortSetup = (&'static str, Box<dyn Fn(&World)>);

/// Every case.
fn cases(world: &World) -> Vec<Case> {
    let repo = &world.repo;
    let full = r#","context_window":{"current_context_tokens":45678,"displayed_context_limit":200000,"current_context_used_percentage":22.8,"used_percentage":91},"ai_used":{"formatted":"12.3"},"cost":{"total_premium_requests":1.5,"total_duration_ms":3723000}"#;
    let mut cases = vec![
        (
            "full, ok".into(),
            payload(repo, full),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "full, fyis".into(),
            payload(repo, full),
            Some("%9"),
            Some(OK_FYI),
            None,
        ),
        (
            "dead seat".into(),
            payload(repo, full),
            Some("%9"),
            Some(DEAD),
            None,
        ),
        (
            "unregistered".into(),
            payload(repo, full),
            Some("%9"),
            Some(UNREGISTERED),
            None,
        ),
        (
            "daemon down (no output)".into(),
            payload(repo, full),
            Some("%9"),
            None,
            None,
        ),
        (
            "malformed ok".into(),
            payload(repo, full),
            Some("%9"),
            Some(MALFORMED_OK),
            None,
        ),
        (
            "empty TMUX_PANE, pij id".into(),
            payload(repo, full),
            Some(""),
            None,
            Some(r#"{"ok":true,"data":{"id":"pij-plain"}}"#),
        ),
        (
            "no pane, pij id".into(),
            payload(repo, full),
            None,
            None,
            Some(r#"{"ok":true,"data":{"id":"pij-plain"}}"#),
        ),
        (
            "no pane, legacy top-level id".into(),
            payload(repo, full),
            None,
            None,
            Some(r#"{"id":"pij-legacy"}"#),
        ),
        (
            "no pane, nothing".into(),
            payload(repo, full),
            None,
            None,
            None,
        ),
        ("minimal {}".into(), "{}".into(), Some("%9"), Some(OK), None),
        (
            "empty stdin".into(),
            String::new(),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "invalid json".into(),
            "{not json".into(),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "type error: context_window is a number".into(),
            payload(repo, r#","context_window":5"#),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "type error: tokens are a string".into(),
            payload(repo, r#","context_window":{"current_context_tokens":"12"}"#),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "percent only".into(),
            payload(
                repo,
                r#","context_window":{"current_context_used_percentage":37.5,"context_window_size":128000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "last-call percent only".into(),
            payload(repo, r#","context_window":{"used_percentage":10}"#),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "zero limit falls back".into(),
            payload(
                repo,
                r#","context_window":{"displayed_context_limit":0,"current_context_tokens":1000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "millions tie".into(),
            payload(
                repo,
                r#","context_window":{"current_context_tokens":1250000,"displayed_context_limit":2000000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "millions".into(),
            payload(
                repo,
                r#","context_window":{"current_context_tokens":1350000,"displayed_context_limit":1000000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "thousands tie".into(),
            payload(
                repo,
                r#","context_window":{"current_context_tokens":1250,"displayed_context_limit":12500}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "tens of thousands".into(),
            payload(
                repo,
                r#","context_window":{"current_context_tokens":99999,"displayed_context_limit":10000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "under a thousand".into(),
            payload(
                repo,
                r#","context_window":{"current_context_tokens":999.9,"displayed_context_limit":999}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "premium zero, short session".into(),
            payload(
                repo,
                r#","cost":{"total_premium_requests":0,"total_duration_ms":59999}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "premium fraction, AIU text".into(),
            payload(
                repo,
                r#","ai_used":{"formatted":"1,234.5"},"cost":{"total_premium_requests":0.33,"total_duration_ms":61000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "float duration is not an integer".into(),
            payload(repo, r#","cost":{"total_duration_ms":1234.5}"#),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "AIU with escapes".into(),
            payload(
                repo,
                r#","ai_used":{"formatted":"a\\tb\\cZ"},"cost":{"total_duration_ms":7200000}"#,
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "model id only, cwd field".into(),
            format!(
                r#"{{"cwd":{},"model":{{"id":"gpt-6"}}}}"#,
                serde_json::to_string(world.plain.to_str().expect("utf8")).expect("json")
            ),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "root dir".into(),
            r#"{"workspace":{"current_dir":"/"}}"#.into(),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "trailing slash".into(),
            format!(r#"{{"workspace":{{"current_dir":"{}/"}}}}"#, repo.display()),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "repo subdir".into(),
            payload(&repo.join("sub/dir"), ""),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "linked worktree".into(),
            payload(&world.worktree, ""),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "detached HEAD".into(),
            payload(&world.detached, ""),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "branch name shared with a tag".into(),
            payload(&world.ambiguous, ""),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "not a repo".into(),
            payload(&world.plain, ""),
            Some("%9"),
            Some(OK),
            None,
        ),
        (
            "missing dir".into(),
            payload(&world.root.join("absent"), ""),
            Some("%9"),
            Some(OK),
            None,
        ),
    ];
    cases.push((
        "relative cwd (.)".into(),
        r#"{"workspace":{"current_dir":"."}}"#.into(),
        Some("%9"),
        Some(OK),
        None,
    ));
    cases
}

#[test]
fn every_golden_case_matches_the_plan_156_script_byte_for_byte() {
    if !(have("jq") && have("git") && Path::new("/bin/bash").exists()) {
        eprintln!("SKIP: the plan-156 script needs bash, jq and git");
        return;
    }
    let world = World::new();
    let mut failures = Vec::new();
    // Effort file variants, applied in turn across the whole corpus.
    let effort_setups: [EffortSetup; 6] = [
        ("no settings", Box::new(|_| {})),
        (
            "home high",
            Box::new(|w: &World| {
                w.write(
                    &w.home.join(".copilot/settings.json"),
                    r#"{"effortLevel":"high","x":1}"#,
                )
            }),
        ),
        (
            "pwd medium wins",
            Box::new(|w: &World| {
                w.write(
                    &w.pwd.join(".github/copilot/settings.json"),
                    r#"{"effortLevel":"medium"}"#,
                )
            }),
        ),
        (
            "local low wins",
            Box::new(|w: &World| {
                w.write(
                    &w.pwd.join(".github/copilot/settings.local.json"),
                    r#"{"effortLevel":"low"}"#,
                )
            }),
        ),
        (
            "local invalid, null pwd -> home",
            Box::new(|w: &World| {
                w.write(&w.pwd.join(".github/copilot/settings.local.json"), "{oops");
                w.write(
                    &w.pwd.join(".github/copilot/settings.json"),
                    r#"{"effortLevel":null}"#,
                );
            }),
        ),
        (
            "numeric effort",
            Box::new(|w: &World| {
                w.write(
                    &w.pwd.join(".github/copilot/settings.json"),
                    r#"{"effortLevel":3}"#,
                )
            }),
        ),
    ];
    let all = cases(&world);
    let mut renders = 0;
    for (index, (effort_label, setup)) in effort_setups.iter().enumerate() {
        setup(&world);
        // The whole corpus once; each later effort setup on three cases.
        let subset: Vec<_> = if index == 0 {
            all.clone()
        } else {
            all.iter()
                .take(1)
                .chain(all.iter().skip(10).take(2))
                .cloned()
                .collect()
        };
        for (label, payload, pane, whoami, pij) in subset {
            renders += 1;
            let expected = world.script(&payload, pane, whoami, pij);
            let actual = world.rust(&payload, pane, whoami, pij);
            if expected != actual.as_bytes() {
                failures.push(format!(
                    "[{effort_label}] {label}\n  script: {}\n  rust:   {}",
                    escape(&expected),
                    escape(actual.as_bytes())
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {renders} renders differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Honest by construction (plan 156 rule 5): the render shows what THIS
/// render's whoami said. A seat that died between two refreshes shows dead on
/// the next one; nothing is cached across renders.
#[test]
fn a_seat_that_dies_shows_dead_on_the_very_next_render() {
    let context = |whoami: &str| Context {
        pwd: std::env::temp_dir(),
        home: None,
        whoami: Whoami::Pane(whoami.to_string()),
    };
    let none = |_: &Path| None;
    let alive = render_copilot(b"{}", &context(OK), &none);
    let dead = render_copilot(b"{}", &context(DEAD), &none);
    assert!(alive.contains("⛓ pij-brave-otter"), "{alive}");
    assert!(dead.contains("⛓ pij-gone-heron dead"), "{dead}");
    assert!(
        !dead.contains("pij-brave-otter"),
        "no earlier answer leaks: {dead}"
    );
}

#[test]
fn the_common_branch_case_launches_no_git() {
    let world = World::new();
    // The fast path reads repository files only; git runs in the fallback.
    assert_eq!(
        super::plain_branch(&world.repo).and_then(|plain| match plain {
            super::Plain::Branch(name) => Some(name),
            super::Plain::NotARepo => None,
        }),
        Some("feat/spawn-storm".to_string())
    );
    assert_eq!(
        super::plain_branch(&world.worktree).and_then(|plain| match plain {
            super::Plain::Branch(name) => Some(name),
            super::Plain::NotARepo => None,
        }),
        Some("wt-branch".to_string())
    );
    assert!(matches!(
        super::plain_branch(&world.plain),
        Some(super::Plain::NotARepo)
    ));
    assert!(
        super::plain_branch(&world.detached).is_none(),
        "detached HEAD asks git"
    );
    assert!(
        super::plain_branch(&world.ambiguous).is_none(),
        "an ambiguous name asks git"
    );
}
