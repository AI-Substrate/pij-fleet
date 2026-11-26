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
    let mut command = pij_testkit::git_env::git(dir);
    let output = pij_testkit::git_env::scrub_identity(&mut command)
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
        let _ = pij_testkit::git_env::git(&self.repo)
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
/// render's whoami said, with nothing cached across renders. A seat that died
/// between two refreshes is not shown on the next one, by name or as dead
/// (F198): a refusal never names the seat it mentions.
#[test]
fn a_seat_that_dies_is_not_named_on_the_very_next_render() {
    let context = |whoami: &str| Context {
        pwd: std::env::temp_dir(),
        home: None,
        whoami: Whoami::Pane(whoami.to_string()),
    };
    let none = |_: &Path| None;
    let alive = render_copilot(b"{}", &context(OK), &none);
    let dead = render_copilot(b"{}", &context(DEAD), &none);
    assert!(alive.contains("⛓ pij-brave-otter"), "{alive}");
    assert!(dead.contains("⛓ unregistered"), "{dead}");
    assert!(
        !dead.contains("pij-brave-otter") && !dead.contains("pij-gone-heron"),
        "no seat is named: {dead}"
    );
}

/// F198: an unregistered Copilot session shares a cwd, a pane id (another tmux
/// server's `%9`) and an inherited `PIJ_SESSION_ID` with registered seats, live
/// and dead. Its status line names none of them, and names its own seat only
/// when the seat records this session's process (pid AND start).
#[test]
fn an_unregistered_session_never_shows_another_seats_name() {
    use pij_core::model::ProcIdentity;

    use super::{pane_whoami, plain_id, runs_under};

    // This status line (pid 500) runs under the Copilot CLI (pid 400), which
    // the npm loader (node, 300) started.
    let copilot = ProcIdentity {
        pid: 400,
        proc_start: 20_261_118_090_000,
    };
    let process = |pid: u32| match pid {
        500 => Some((400, "pij-rs".to_string())),
        400 => Some((300, "copilot".to_string())),
        300 => Some((1, "node".to_string())),
        _ => None,
    };
    let start = |pid: u32| (pid == 400).then_some(copilot.proc_start);
    let owns = |seat: ProcIdentity| runs_under(seat, 500, &process, &start);
    let cwd = std::env::temp_dir();
    let answer = |id: &str, proc: ProcIdentity| {
        serde_json::json!({
            "ok": true,
            "command": "pij whoami",
            "data": {
                "id": id,
                "harness": "copilot",
                "pane": "%9",
                "folder": cwd.display().to_string(),
                "proc": proc,
                "pending_fyis": 2,
            },
        })
        .to_string()
    };
    let render = |whoami: Whoami| {
        render_copilot(
            payload(&cwd, "").as_bytes(),
            &Context {
                pwd: cwd.clone(),
                home: None,
                whoami,
            },
            &|_: &Path| None,
        )
    };

    // A live seat on the same pane id and cwd, in another process.
    let porcupine = answer(
        "pij-possible-porcupine",
        ProcIdentity {
            pid: 4242,
            proc_start: 20_261_117_080_000,
        },
    );
    let held = render(pane_whoami(porcupine.clone(), &owns));
    assert!(held.contains("⛓ unregistered (pane held)"), "{held}");
    assert!(!held.contains("porcupine") && !held.contains('✉'), "{held}");
    // The same seat reached through an inherited (stale) PIJ_SESSION_ID.
    assert_eq!(plain_id(&porcupine, &owns), None);
    let plain = render(Whoami::Plain(plain_id(&porcupine, &owns)));
    assert!(plain.contains("⛓ unregistered") && !plain.contains("porcupine"));

    // A dead seat whose recorded pid is now this session's own Copilot: a
    // recycled pid is someone else.
    let recycled = answer(
        "pij-monthly-nenneke",
        ProcIdentity {
            pid: 400,
            proc_start: 20_261_101_000_000,
        },
    );
    let shown = render(pane_whoami(recycled.clone(), &owns));
    assert!(!shown.contains("nenneke"), "{shown}");
    assert_eq!(plain_id(&recycled, &owns), None);
    // ... or refused as tombstoned: still never named.
    let tombstoned = r#"{"ok":false,"command":"pij whoami","error":{"kind":"refused","message":"seat `pij-monthly-nenneke` is tombstoned and is not an identity"}}"#;
    let shown = render(pane_whoami(tombstoned.to_string(), &owns));
    assert!(shown.contains("⛓ unregistered") && !shown.contains("nenneke"));

    // A seat with no recorded process, or a remote one, cannot be shown either.
    let unbound = r#"{"ok":true,"data":{"id":"pij-unbound","pane":"%9"}}"#;
    assert!(matches!(
        pane_whoami(unbound.to_string(), &owns),
        Whoami::PaneHeld
    ));
    let remote = r#"{"ok":true,"data":{"id":"pij-remote","machine":"mini","proc":{"pid":400,"proc_start":20261118090000}}}"#;
    assert!(matches!(
        pane_whoami(remote.to_string(), &owns),
        Whoami::PaneHeld
    ));

    // This session's own seat is named, with its held FYIs.
    let own = answer("pij-own-seat", copilot);
    let shown = render(pane_whoami(own.clone(), &owns));
    assert!(
        shown.contains("⛓ pij-own-seat\u{1b}[0m \u{1b}[33m✉2"),
        "{shown}"
    );
    assert_eq!(plain_id(&own, &owns).as_deref(), Some("pij-own-seat"));
    // Answers that name no seat pass through unchanged.
    assert!(matches!(
        pane_whoami(UNREGISTERED.to_string(), &owns),
        Whoami::Pane(_)
    ));
}

/// S240-001: a second Copilot B started inside registered Copilot A's process
/// tree (A → B → shell wrapper → status line) is not A's seat, on the pane
/// path (B shares A's pane) or the paneless one (B inherits A's
/// `PIJ_SESSION_ID`). Only the NEAREST host counts; wrappers are passed over.
#[test]
fn a_copilot_nested_in_a_registered_one_never_shows_its_name() {
    use pij_core::model::ProcIdentity;

    use super::{pane_whoami, plain_id, runs_under};

    let a = ProcIdentity {
        pid: 100,
        proc_start: 20_261_118_080_000,
    };
    let b = ProcIdentity {
        pid: 200,
        proc_start: 20_261_118_090_000,
    };
    // 400: A's own status line. 300: B's, behind a shell wrapper (250).
    // 350: B's, launched directly.
    let process = |pid: u32| match pid {
        400 => Some((100, "pij-rs".to_string())),
        350 => Some((200, "pij-rs".to_string())),
        300 => Some((250, "pij-rs".to_string())),
        250 => Some((200, "sh".to_string())),
        200 => Some((100, "copilot".to_string())),
        100 => Some((50, "copilot".to_string())),
        50 => Some((1, "zsh".to_string())),
        _ => None,
    };
    let start = |pid: u32| match pid {
        100 => Some(a.proc_start),
        200 => Some(b.proc_start),
        _ => None,
    };
    let seat = |id: &str, proc: ProcIdentity| {
        serde_json::json!({"ok": true, "data": {"id": id, "pane": "%9", "proc": proc}}).to_string()
    };
    let registered_a = seat("pij-registered-a", a);
    for status_line in [300, 350] {
        let owns = |proc: ProcIdentity| runs_under(proc, status_line, &process, &start);
        // The pane path: B in A's pane is answered with A.
        match pane_whoami(registered_a.clone(), &owns) {
            Whoami::PaneHeld => {}
            other => panic!("B's status line {status_line} named A: {other:?}"),
        }
        // The paneless path: B inherited A's PIJ_SESSION_ID.
        assert_eq!(plain_id(&registered_a, &owns), None);
        // Once B registers, its own seat shows, wrapper or not.
        assert_eq!(
            plain_id(&seat("pij-registered-b", b), &owns).as_deref(),
            Some("pij-registered-b")
        );
    }
    // A's own status line, a direct child, still shows A.
    let owns = |proc: ProcIdentity| runs_under(proc, 400, &process, &start);
    assert_eq!(
        plain_id(&registered_a, &owns).as_deref(),
        Some("pij-registered-a")
    );
}

/// The in-process reader the walk uses names this test process and its real
/// parent (macOS `p_comm`, Linux `/proc/<pid>/exe`), launching nothing.
#[test]
fn the_host_reader_names_this_process_and_its_real_parent() {
    let (parent, name) =
        pij_harnesses::proc::parent_and_host_name(std::process::id()).expect("readable");
    assert_eq!(parent, std::os::unix::process::parent_id());
    assert!(name.starts_with("pij_cli"), "{name}");
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

/// Plan 158: `✉N` beside the seat only while N > 0, read from the same whoami.
#[test]
fn held_fyis_show_only_while_some_are_pending() {
    let render = |whoami: &str| {
        render_copilot(
            b"{}",
            &Context {
                pwd: std::env::temp_dir(),
                home: None,
                whoami: Whoami::Pane(whoami.to_string()),
            },
            &|_: &Path| None,
        )
    };
    let held = render(r#"{"ok":true,"data":{"id":"pij-fyi-seat","pending_fyis":3}}"#);
    assert!(
        held.contains("⛓ pij-fyi-seat\u{1b}[0m \u{1b}[33m✉3"),
        "{held}"
    );
    for quiet in [
        r#"{"ok":true,"data":{"id":"pij-fyi-seat","pending_fyis":0}}"#,
        r#"{"ok":true,"data":{"id":"pij-fyi-seat"}}"#,
    ] {
        let rendered = render(quiet);
        assert!(rendered.contains("⛓ pij-fyi-seat"), "{rendered}");
        assert!(!rendered.contains('✉'), "{rendered}");
    }
}

/// Plan 156 AC5, moved here with the renderer: for a live seat the managed
/// status line keeps Jordan's hand-written layout byte for byte.
#[test]
fn a_live_seat_keeps_the_hand_written_layout() {
    if !(have("jq") && have("git")) {
        eprintln!("SKIP: the hand-written script needs jq and git");
        return;
    }
    let world = World::new();
    let hand_written = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../harnesses/tests/fixtures/copilot-statusline-context.sh.txt"
    );
    let hit = r#"{"ok":true,"data":{"id":"pij-copilot-seat"}}"#;
    let payload = format!(
        r#"{{"workspace":{{"current_dir":"{}"}},"model":{{"display_name":"GPT-5.6"}},"context_window":{{"current_context_tokens":46133,"displayed_context_limit":400000}},"ai_used":{{"formatted":"1.2"}},"cost":{{"total_premium_requests":3,"total_duration_ms":3720000}}}}"#,
        world.repo.display()
    );
    let _ = fs::remove_file(world.root.join("whoami.txt"));
    world.write(&world.root.join("pij.txt"), hit);
    let mut child = Command::new("/bin/bash")
        .arg(hand_written)
        .env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}",
                world.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &world.home)
        .env("TMUX_PANE", "%21")
        .current_dir(&world.pwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("bash");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("payload");
    let expected = child.wait_with_output().expect("hand-written").stdout;
    let actual = world.rust(&payload, Some("%21"), Some(hit), None);
    assert_eq!(escape(actual.as_bytes()), escape(&expected));
}
