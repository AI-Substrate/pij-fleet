//! Plan 156 AC4 — the Claude status line and SessionStart hook heal a seat.
//!
//! Script-level: a fake `pij-rs` on PATH answers `whoami` with a miss, records
//! every `adopt`, and takes 3 s to do it, so a render that waited on the heal
//! would be visibly slow. `ping` succeeds only once `daemon-up` exists, so the
//! hook's background retry can be shown to outlast a late daemon.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FAKE_PIJ_RS: &str = r#"#!/bin/sh
dir=$(dirname "$0")
case " $* " in
  *" whoami "*) printf '%s\n' '{"ok":false,"error":"refused"}'; exit 1 ;;
  *" ping "*|"ping "*) [ -f "$dir/daemon-up" ] ;;
  *" adopt "*|"adopt "*) printf '%s\n' "$*" >>"$dir/adopts"; sleep 3 ;;
esac
"#;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/scripts")
        .join(name)
}

fn run(root: &Path, name: &str, payload: &str) -> (String, Duration) {
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    let started = Instant::now();
    let mut child = Command::new("sh")
        .arg(script(name))
        .env("PATH", path)
        .env("HOME", root)
        .env("PIJ_RS_STATE_DIR", root.join("state"))
        .env("TMUX_PANE", "%9")
        .env("PIJ_RS_HOOK_RETRIES", "8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run script");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    // The caller (Claude) reads to EOF: a background job holding stdout would
    // block here, which is exactly the render stall this test forbids.
    let output = child.wait_with_output().expect("script output");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        started.elapsed(),
    )
}

fn adopts(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("bin/adopts"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn status_line_fires_one_detached_adopt_per_window_and_hook_outlasts_a_late_daemon() {
    let root = pij_testkit::fresh_dir("pij-claude-heal");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    // The legacy CLI is shadowed with a miss: the real one would consult the
    // operator's legacy daemon about pane %9.
    for (name, body) in [("pij-rs", FAKE_PIJ_RS), ("pij", "#!/bin/sh\nexit 1\n")] {
        let fake = root.join("bin").join(name);
        std::fs::write(&fake, body).unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
    }
    let payload = format!(
        r#"{{"session_id":"S-156","workspace":{{"current_dir":"{}"}},"model":{{"display_name":"Opus"}}}}"#,
        root.display()
    );

    // Three renders inside one window: each is fast, says it is registering,
    // and together they fire exactly one adopt carrying the payload session.
    for _ in 0..3 {
        let (rendered, took) = run(&root, "claude-statusline-pij.sh", &payload);
        assert!(
            took < Duration::from_secs(2),
            "render waited on the heal: {took:?}"
        );
        assert!(rendered.contains("registering"), "{rendered}");
    }
    eventually("the status-line adopt", || !adopts(&root).is_empty());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        adopts(&root),
        ["adopt %9 --harness claude --harness-session S-156"],
        "one adopt per pane per rate window"
    );

    // The hook, with no daemon yet: returns at once, then adopts once it appears.
    std::fs::remove_file(root.join("bin/adopts")).unwrap();
    let (_, took) = run(&root, "claude-session-start-pij.sh", &payload);
    assert!(
        took < Duration::from_secs(2),
        "hook held Claude's start: {took:?}"
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        adopts(&root).is_empty(),
        "no adopt before the daemon exists"
    );
    std::fs::write(root.join("bin/daemon-up"), "").unwrap();
    eventually("the hook's retried adopt", || !adopts(&root).is_empty());

    let log = std::fs::read_to_string(root.join("state/hook.log")).unwrap();
    assert!(log.lines().count() >= 1, "{log}");
    for line in log.lines() {
        let stamp = line.split(' ').next().unwrap();
        assert!(
            stamp.len() == 20 && stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
            "every line is RFC 3339 stamped: {line}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Plan 156 AC5 — the pij-managed Copilot status line keeps Jordan's layout and
/// is honest about a dead or missing seat.
#[test]
fn copilot_status_line_matches_the_hand_written_layout_and_names_dead_seats() {
    let root = pij_testkit::fresh_dir("pij-copilot-statusline");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let hit = r#"{"ok":true,"data":{"id":"pij-copilot-seat"}}"#;
    let write = |name: &str, answer: &str| {
        let fake = root.join("bin").join(name);
        std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' '{answer}'\n")).unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
    };
    write("pij", hit);
    let payload = format!(
        r#"{{"workspace":{{"current_dir":"{}"}},"model":{{"display_name":"GPT-5.6"}},"context_window":{{"current_context_tokens":46133,"displayed_context_limit":400000}},"ai_used":{{"formatted":"1.2"}},"cost":{{"total_premium_requests":3,"total_duration_ms":3720000}}}}"#,
        root.display()
    );
    let render = |script: &Path| {
        let path = format!(
            "{}:{}",
            root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut child = Command::new("bash")
            .arg(script)
            .current_dir(&root)
            .env("PATH", path)
            .env("HOME", &root)
            .env("TMUX_PANE", "%21")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("render");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let hand_written = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/copilot-statusline-context.sh.txt");
    let installed = pij_harnesses::install_copilot_statusline_script(&root.join("state")).unwrap();

    write("pij-rs", hit);
    assert_eq!(render(&installed), render(&hand_written), "layout survives");
    assert!(render(&installed).contains("⛓ pij-copilot-seat"));

    write(
        "pij-rs",
        r#"{"ok":false,"error":"refused","meta":"seat `pij-copilot-seat` records pane `%21`, but its process (pid 1, start 2) is gone"}"#,
    );
    let dead = render(&installed);
    assert!(dead.contains("⛓ pij-copilot-seat dead"), "{dead}");
    write(
        "pij-rs",
        r#"{"ok":false,"error":"refused","meta":"no live seat"}"#,
    );
    assert!(render(&installed).contains("⛓ unregistered"));
    let _ = std::fs::remove_dir_all(root);
}

// Plan 174 (pij-fleet#55): the double-seat warning. The daemon refuses a second
// copy of a live conversation with E-RS-CONVERSATION-LIVE; every Claude script
// turns that refusal into something the human and the model can see.

const CODE: &str = "E-RS-CONVERSATION-LIVE";
/// The daemon's refusal line, as `pij-rs` prints it for the stray pane %9.
const CODED: &str = "E-RS-CONVERSATION-LIVE conversation `S-174` is live as seat \
                     `pij-different-smelt` in pane %5; close one copy, then run \
                     `pij-rs adopt %9 --harness claude` from the copy you keep \
                     (one conversation cannot be two seats)";
const RED_DOUBLE: &str = "\u{1b}[31m⚠ double: pij-different-smelt in %5\u{1b}[0m";

/// A fake `pij-rs` that answers every verb from files: `<verb>.out` on stdout
/// and `<verb>.status` as the exit code. `adopt` also appends its argv to
/// `adopts`.
const SCRIPTED_PIJ_RS: &str = r#"#!/bin/sh
dir=$(dirname "$0")
verb=""
for arg; do
  case $arg in whoami|ping|adopt|activity|fyi-claim) verb=$arg; break ;; esac
done
[ "$verb" = adopt ] && printf '%s\n' "$*" >>"$dir/adopts"
[ -f "$dir/$verb.out" ] && cat "$dir/$verb.out"
exit "$(cat "$dir/$verb.status" 2>/dev/null || echo 0)"
"#;

struct Bench {
    root: PathBuf,
}

impl Bench {
    fn new(prefix: &str) -> Self {
        let root = pij_testkit::fresh_dir(prefix);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        // The legacy CLI is shadowed with a miss, as above.
        for (name, body) in [("pij-rs", SCRIPTED_PIJ_RS), ("pij", "#!/bin/sh\nexit 1\n")] {
            let fake = root.join("bin").join(name);
            std::fs::write(&fake, body).unwrap();
            let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&fake, permissions).unwrap();
        }
        Self { root }
    }

    fn answer(&self, verb: &str, status: u8, out: &str) {
        std::fs::write(self.root.join(format!("bin/{verb}.out")), out).unwrap();
        std::fs::write(
            self.root.join(format!("bin/{verb}.status")),
            status.to_string(),
        )
        .unwrap();
    }

    /// Run `name` as Claude would: payload on stdin, stdout read to EOF.
    fn run(&self, name: &str, payload: &str) -> (i32, String, Duration) {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let started = Instant::now();
        let mut child = Command::new("sh")
            .arg(script(name))
            .env("PATH", path)
            .env("HOME", &self.root)
            .env("PIJ_RS_STATE_DIR", self.root.join("state"))
            .env("TMUX_PANE", "%9")
            .env_remove("PIJ_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("run script");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().expect("script output");
        (
            output.status.code().expect("exit code"),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            started.elapsed(),
        )
    }

    fn render(&self, session: &str) -> String {
        let payload = format!(
            r#"{{"session_id":"{session}","workspace":{{"current_dir":"{}"}},"model":{{"display_name":"Opus"}}}}"#,
            self.root.display()
        );
        let (status, rendered, took) = self.run("claude-statusline-pij.sh", &payload);
        assert_eq!(status, 0, "{rendered}");
        assert!(
            took < Duration::from_secs(2),
            "render left today's budget: {took:?}"
        );
        rendered
    }

    fn heal(&self, suffix: &str) -> PathBuf {
        self.root.join(format!("state/heal/pane-9.{suffix}"))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("state/hook.log")).unwrap_or_default()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Hook output must parse as ONE JSON document, or Claude shows it as noise.
fn hook_json(event: &str, stdout: &str) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("hook output must be JSON ({error}): {stdout:?}"));
    assert_eq!(
        value["hookSpecificOutput"]["hookEventName"], event,
        "{value}"
    );
    value
}

/// Both faces, the human's `systemMessage` and the model's
/// `additionalContext`, name the live seat, its pane and the fix.
fn assert_warns(value: &serde_json::Value) {
    for (face, text) in [
        ("systemMessage", &value["systemMessage"]),
        (
            "additionalContext",
            &value["hookSpecificOutput"]["additionalContext"],
        ),
    ] {
        let text = text.as_str().unwrap_or_default();
        for part in [
            "pij-different-smelt",
            "pane %5",
            "pij-rs adopt %9 --harness claude",
        ] {
            assert!(text.contains(part), "{face} must name `{part}`: {value}");
        }
    }
}

#[test]
fn session_start_hook_warns_human_and_model_when_adopt_finds_a_double() {
    let bench = Bench::new("pij-double-session-start");
    bench.answer("ping", 0, "");
    bench.answer("adopt", 1, &format!("pij adopt: FAILED — {CODED}\n"));

    let (status, stdout, took) =
        bench.run("claude-session-start-pij.sh", r#"{"session_id":"S-174"}"#);

    assert_eq!(status, 0, "the hook never blocks Claude's start");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_warns(&hook_json("SessionStart", &stdout));
    let log = bench.log();
    assert!(
        log.contains("refused:adopt pane=%9") && log.contains(CODE),
        "the hook.log line stays: {log}"
    );

    // Any other refusal keeps today's behaviour: log only.
    bench.answer("adopt", 1, "pij adopt: FAILED — the daemon is busy\n");
    let (status, stdout, _) = bench.run("claude-session-start-pij.sh", r#"{"session_id":"S-174"}"#);
    assert_eq!(status, 0);
    assert_eq!(stdout, "", "an uncoded refusal prints nothing");
    assert_eq!(bench.log().lines().count(), 2, "{}", bench.log());
}

#[test]
fn user_prompt_hook_warns_on_every_prompt_while_the_double_persists() {
    let bench = Bench::new("pij-double-prompt");
    bench.answer("activity", 1, &format!("pij activity: FAILED — {CODED}\n"));
    bench.answer(
        "fyi-claim",
        1,
        &format!("pij fyi-claim: FAILED — {CODED}\n"),
    );
    let payload = r#"{"session_id":"S-174","prompt":"hello"}"#;

    for _ in 0..2 {
        let (status, stdout, _) = bench.run("claude-user-prompt-submit-pij.sh", payload);
        assert_eq!(status, 0, "a prompt is never blocked");
        assert_warns(&hook_json("UserPromptSubmit", &stdout));
    }

    // The double clears: an ordinary miss adds nothing.
    bench.answer(
        "activity",
        1,
        "pij activity: FAILED — no live seat is bound to pane %9\n",
    );
    bench.answer(
        "fyi-claim",
        1,
        "pij fyi-claim: FAILED — no live seat is bound to pane %9\n",
    );
    let (status, stdout, _) = bench.run("claude-user-prompt-submit-pij.sh", payload);
    assert_eq!((status, stdout.as_str()), (0, ""));

    // A registered pane's FYI claim is unchanged, byte for byte.
    bench.answer("activity", 0, "");
    bench.answer("fyi-claim", 0, "FYI one\nFYI two\n");
    let (status, stdout, _) = bench.run("claude-user-prompt-submit-pij.sh", payload);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        "{\"hookSpecificOutput\":{\"hookEventName\":\"UserPromptSubmit\",\"additionalContext\":\"FYI one\\nFYI two\"}}\n"
    );
}

#[test]
fn status_line_shows_the_double_from_whoami_and_fires_no_adopt() {
    let bench = Bench::new("pij-double-whoami");
    bench.answer(
        "whoami",
        1,
        &format!(
            r#"{{"ok":false,"command":"pij whoami","v":1,"meta":"{CODED}","error":"refused","details":{{"code":"{CODE}","seat":"pij-different-smelt","pane":"%5","harness_session":"S-174","pid":3999951}}}}"#
        ),
    );
    bench.answer("adopt", 1, &format!("pij adopt: FAILED — {CODED}\n"));

    let rendered = bench.render("S-174");

    assert!(rendered.contains(RED_DOUBLE), "{rendered:?}");
    assert!(!rendered.contains("registering"), "{rendered:?}");
    assert!(
        !rendered.contains('⛓'),
        "the double replaces the seat tag: {rendered:?}"
    );
    assert!(
        !bench.heal("last").exists(),
        "no heal adopt was scheduled for a coded whoami"
    );
}

#[test]
fn status_line_remembers_a_coded_heal_refusal_and_never_fires_a_second_adopt() {
    let bench = Bench::new("pij-double-heal");
    bench.answer(
        "whoami",
        1,
        r#"{"ok":false,"command":"pij whoami","v":1,"meta":"no live seat in the rs store owns pane %9","error":"refused"}"#,
    );
    bench.answer("adopt", 1, &format!("pij adopt: FAILED — {CODED}\n"));
    // The background heal's one log line is the deterministic completion
    // signal: a FIFO read finishes exactly when the heal has written it.
    let fifo = bench.root.join("state/hook.log");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success(),
        "mkfifo"
    );
    let (sent, heal_logged) = std::sync::mpsc::channel();
    let reader = fifo.clone();
    std::thread::spawn(move || {
        let _ = sent.send(std::fs::read_to_string(reader));
    });

    // First render: nothing known yet, so the heal fires once.
    assert!(bench.render("S-174").contains("registering"));
    let line = heal_logged
        .recv_timeout(Duration::from_secs(10))
        .expect("the heal adopt finished")
        .unwrap();
    assert!(
        line.contains("statusline:refused") && line.contains(CODE),
        "{line}"
    );
    // A plain file from here on: a stray later heal must never block on it.
    std::fs::remove_file(&fifo).unwrap();
    std::fs::write(&fifo, "").unwrap();
    assert!(bench.heal("double").exists(), "the refusal is remembered");

    // The rate window has passed: a render would heal again, but must not.
    std::fs::write(bench.heal("last"), "0\n").unwrap();
    let rendered = bench.render("S-174");
    assert!(rendered.contains(RED_DOUBLE), "{rendered:?}");
    assert!(!rendered.contains("registering"), "{rendered:?}");
    assert_eq!(
        std::fs::read_to_string(bench.heal("last")).unwrap(),
        "0\n",
        "a second adopt was scheduled for the same refusal"
    );

    // The fix (a manual adopt) registers the pane: the warning clears.
    bench.answer(
        "whoami",
        0,
        r#"{"ok":true,"command":"pij whoami","v":1,"data":{"id":"pij-different-smelt","session":"S-174"}}"#,
    );
    let rendered = bench.render("S-174");
    assert!(rendered.contains("⛓ pij-different-smelt"), "{rendered:?}");
    assert!(!rendered.contains("⚠ double"), "{rendered:?}");
    assert!(!bench.heal("double").exists(), "the memory is cleared");
}
