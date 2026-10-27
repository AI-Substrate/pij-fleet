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

/// Plan 180: the pij-managed Copilot status line is `exec pij-rs statusline
/// copilot`, so its layout, dead-seat and FYI behaviour are pij-rs's (golden
/// tested in pij-cli against the plan-156 script and Jordan's hand-written one).
/// Here: the script hands its argv and stdin to pij-rs, in its own process, and
/// says so plainly when pij-rs is missing.
#[test]
fn copilot_status_line_hands_the_payload_to_pij_rs_in_one_process() {
    let root = pij_testkit::fresh_dir("pij-copilot-statusline");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let record = root.join("record");
    let fake = root.join("bin").join("pij-rs");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s|%s|' \"$$\" \"$*\" > '{0}'\ncat >> '{0}'\nprintf RENDERED",
            record.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let payload = r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"GPT-5.6"}}"#;
    let installed = pij_harnesses::install_copilot_statusline_script(&root.join("state")).unwrap();
    let render = |path: &str| {
        let mut child = Command::new("sh")
            .arg(&installed)
            .current_dir(&root)
            .env("PATH", path)
            .env("HOME", &root)
            .env("TMUX_PANE", "%21")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("render");
        let pid = child.id();
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        (pid, out)
    };
    let (pid, out) = render(&format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap()
    ));
    assert_eq!(out, "RENDERED");
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(
        recorded,
        format!("{pid}|statusline copilot|{payload}"),
        "exec (same pid, no fork), exact argv, the payload untouched"
    );
    let (_, missing) = render("/usr/bin:/bin");
    assert!(missing.contains("pij-rs not on PATH"), "{missing}");
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// Plan 174 (pij-fleet#55) — a second copy of a seat's conversation.
//
// A fake `pij-rs` answers with the daemon's E-RS-CONVERSATION-LIVE refusal,
// exactly as the real CLI prints it: human form on stdout for `adopt`,
// `activity` and `fyi-claim`, the raw envelope for `whoami --json`.
// ---------------------------------------------------------------------------

/// The code the scripts match. Pinned to the Rust constant below.
const CODE: &str = "E-RS-CONVERSATION-LIVE";

fn double_meta() -> String {
    format!(
        "{CODE} conversation `S-174` is live as seat `pij-different-smelt` in another process \
         (pane %5); close one copy, then run `pij-rs adopt %9 --harness claude` from the copy \
         you keep"
    )
}

fn install(root: &Path, name: &str, body: &str) {
    let fake = root.join("bin").join(name);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(&fake, body).unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();
}

/// Run `name` with the fake bin first on PATH; stdout, exit code, elapsed.
fn hook(root: &Path, name: &str, payload: &str) -> (String, i32, Duration) {
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
        .env("PIJ_RS_HOOK_RETRIES", "1")
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
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code().unwrap_or(-1),
        started.elapsed(),
    )
}

/// The hook JSON a double must produce: shown to the human AND to the model.
fn assert_double_warning(event: &str, stdout: &str) {
    let answer: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("{event}: hook output is not JSON ({error}): {stdout:?}"));
    let human = answer["systemMessage"].as_str().unwrap_or_default();
    let model = answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(answer["hookSpecificOutput"]["hookEventName"], event);
    for (who, text) in [("systemMessage", human), ("additionalContext", model)] {
        for needle in [
            "pij-different-smelt",
            "%5",
            "close one copy",
            "pij-rs adopt %9",
        ] {
            assert!(
                text.contains(needle),
                "{event} {who} names `{needle}`: {stdout}"
            );
        }
    }
}

/// AC3 — SessionStart: a coded adopt refusal warns the human and the model;
/// any other refusal stays log-only. Exit 0 and a log line on every path.
#[test]
fn session_start_hook_warns_human_and_model_about_a_double_seat() {
    let root = pij_testkit::fresh_dir("pij-double-seat-session-start");
    install(
        &root,
        "pij-rs",
        "#!/bin/sh\ndir=$(dirname \"$0\")\ncase \" $* \" in\n  *\" ping \"*) exit 0 ;;\n  *\" adopt \"*) cat \"$dir/refusal\"; exit 1 ;;\nesac\n",
    );
    let refusal = root.join("bin/refusal");
    let payload = r#"{"session_id":"S-174"}"#;

    std::fs::write(&refusal, format!("pij adopt: FAILED — {}\n", double_meta())).unwrap();
    let (stdout, code, took) = hook(&root, "claude-session-start-pij.sh", payload);
    assert_eq!(code, 0, "never blocks Claude's start");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_double_warning("SessionStart", &stdout);

    std::fs::write(
        &refusal,
        "pij adopt: FAILED — pane `%9` is not running anything tmux can see\n",
    )
    .unwrap();
    let (stdout, code, _) = hook(&root, "claude-session-start-pij.sh", payload);
    assert_eq!(code, 0);
    assert_eq!(
        stdout, "",
        "any other refusal keeps today's log-only behaviour"
    );

    let log = std::fs::read_to_string(root.join("state/hook.log")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("refused:adopt"))
            .count(),
        2,
        "both refusals still logged: {log}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// AC4 — UserPromptSubmit: every prompt warns while the double persists; once
/// it clears the hook adds nothing, and a registered pane's FYI block is passed
/// through byte for byte as before.
#[test]
fn user_prompt_hook_warns_every_prompt_while_the_double_persists() {
    let root = pij_testkit::fresh_dir("pij-double-seat-prompt");
    install(
        &root,
        "pij-rs",
        "#!/bin/sh\ndir=$(dirname \"$0\")\ncase \" $* \" in\n  *\" activity \"*|*\" fyi-claim \"*)\n    if [ -f \"$dir/double\" ]; then cat \"$dir/double\"; exit 1; fi\n    case \" $* \" in *\" fyi-claim \"*) [ -f \"$dir/block\" ] && cat \"$dir/block\" ;; esac\n    exit 0 ;;\nesac\n",
    );
    let payload = r#"{"session_id":"S-174","prompt":"hi"}"#;
    std::fs::write(
        root.join("bin/double"),
        format!("pij activity: FAILED — {}\n", double_meta()),
    )
    .unwrap();
    for _ in 0..2 {
        let (stdout, code, _) = hook(&root, "claude-user-prompt-submit-pij.sh", payload);
        assert_eq!(code, 0);
        assert_double_warning("UserPromptSubmit", &stdout);
    }

    std::fs::remove_file(root.join("bin/double")).unwrap();
    let (stdout, code, _) = hook(&root, "claude-user-prompt-submit-pij.sh", payload);
    assert_eq!((stdout.as_str(), code), ("", 0), "cleared: adds nothing");

    std::fs::write(root.join("bin/block"), "[pij] FYI one\n  second line\n").unwrap();
    let (stdout, code, _) = hook(&root, "claude-user-prompt-submit-pij.sh", payload);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "{\"hookSpecificOutput\":{\"hookEventName\":\"UserPromptSubmit\",\"additionalContext\":\"[pij] FYI one\\n  second line\"}}\n",
        "the registered-pane FYI claim output is unchanged"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// AC5 — status line: a coded whoami refusal shows a red double segment in
/// place of the seat tag or "registering", inside today's budget, and never
/// fires the heal adopt for it.
#[test]
fn status_line_shows_a_red_double_segment_and_never_heals_it() {
    let root = pij_testkit::fresh_dir("pij-double-seat-statusline");
    install(
        &root,
        "pij-rs",
        "#!/bin/sh\ndir=$(dirname \"$0\")\ncase \" $* \" in\n  *\" whoami \"*) cat \"$dir/whoami\"; exit 1 ;;\n  *\" adopt \"*) printf '%s\\n' \"$*\" >>\"$dir/adopts\" ;;\nesac\n",
    );
    install(&root, "pij", "#!/bin/sh\nexit 1\n");
    let envelope = serde_json::json!({
        "v": 2, "ok": false, "command": "pij whoami", "error": "refused",
        "meta": double_meta(),
        "details": {"code": CODE, "seat": "pij-different-smelt", "pane": "%5",
                    "harness_session": "S-174", "pid": 917_405},
    });
    std::fs::write(root.join("bin/whoami"), format!("{envelope}\n")).unwrap();
    let payload = format!(
        r#"{{"session_id":"S-174","workspace":{{"current_dir":"{}"}},"model":{{"display_name":"Opus"}}}}"#,
        root.display()
    );

    let (rendered, code, took) = hook(&root, "claude-statusline-pij.sh", &payload);
    assert_eq!(code, 0);
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(
        rendered.contains("\x1b[31m⚠ double: pij-different-smelt in %5"),
        "red double segment: {rendered:?}"
    );
    assert!(!rendered.contains("registering"), "{rendered:?}");
    assert!(!rendered.contains("⛓ pij-different-smelt"), "{rendered:?}");
    // The heal stamps its window synchronously before forking the adopt, so
    // the absence of the stamp proves no adopt was fired, with no waiting.
    assert!(
        !root.join("state/heal/pane-9.last").exists(),
        "a coded refusal must not fire the heal adopt"
    );
    assert!(adopts(&root).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// Cross-runtime literal seam: the scripts match the code as a literal, so the
/// literal they carry must be the one Rust raises (and the one these tests use).
#[test]
fn scripts_spell_the_conversation_live_code_as_rust_defines_it() {
    assert_eq!(CODE, pij_core::double_seat::CONVERSATION_LIVE_CODE);
    for name in [
        "claude-session-start-pij.sh",
        "claude-user-prompt-submit-pij.sh",
        "claude-statusline-pij.sh",
    ] {
        let text = std::fs::read_to_string(script(name)).unwrap();
        assert!(
            text.contains(pij_core::double_seat::CONVERSATION_LIVE_CODE),
            "{name} does not carry the code Rust raises"
        );
    }
}

/// AC5, heal branch (review item 4): rs names this pane as another seat that
/// records a different conversation, so the status line heals, and the heal
/// adopt is refused with the code. That refusal turns the line red, and no
/// further adopt fires for it, even once the 15 s window has passed.
#[test]
fn status_line_heal_refused_as_a_double_shows_red_and_never_adopts_again() {
    let root = pij_testkit::fresh_dir("pij-double-seat-heal");
    install(
        &root,
        "pij-rs",
        "#!/bin/sh\ndir=$(dirname \"$0\")\ncase \" $* \" in\n  *\" whoami \"*) printf '%s\\n' '{\"ok\":true,\"data\":{\"id\":\"pij-other-seat\",\"session\":\"S-older\"}}' ;;\n  *\" adopt \"*) printf '%s\\n' \"$*\" >>\"$dir/adopts\"; cat \"$dir/refusal\"; exit 1 ;;\nesac\n",
    );
    install(&root, "pij", "#!/bin/sh\nexit 1\n");
    std::fs::write(
        root.join("bin/refusal"),
        format!("pij adopt: FAILED — {}\n", double_meta()),
    )
    .unwrap();
    let payload = format!(
        r#"{{"session_id":"S-174","workspace":{{"current_dir":"{}"}},"model":{{"display_name":"Opus"}}}}"#,
        root.display()
    );
    let marker = root.join("state/heal/pane-9.double");

    let (first, code, _) = hook(&root, "claude-statusline-pij.sh", &payload);
    assert_eq!(code, 0);
    assert!(
        first.contains("registering"),
        "the first render heals: {first:?}"
    );
    // The heal is detached by design; wait only for the fake's marker file.
    eventually("the refused heal's double marker", || marker.exists());
    assert_eq!(adopts(&root).len(), 1);

    // Rewind the heal window: a plain refusal would adopt again now.
    std::fs::write(root.join("state/heal/pane-9.last"), "0\n").unwrap();
    let (second, code, took) = hook(&root, "claude-statusline-pij.sh", &payload);
    assert_eq!(code, 0);
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(
        second.contains("\x1b[31m⚠ double: pij-different-smelt in %5"),
        "red double segment from the heal refusal: {second:?}"
    );
    assert!(!second.contains("registering"), "{second:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("state/heal/pane-9.last")).unwrap(),
        "0\n",
        "no second heal adopt was fired"
    );
    assert_eq!(adopts(&root).len(), 1);

    // rs answers with this conversation (the human adopted after closing a
    // copy): the marker is cleared and the seat tag returns.
    install(
        &root,
        "pij-rs",
        "#!/bin/sh\ncase \" $* \" in\n  *\" whoami \"*) printf '%s\\n' '{\"ok\":true,\"data\":{\"id\":\"pij-different-smelt\",\"session\":\"S-174\"}}' ;;\nesac\n",
    );
    let (third, _, _) = hook(&root, "claude-statusline-pij.sh", &payload);
    assert!(third.contains("⛓ pij-different-smelt"), "{third:?}");
    assert!(!marker.exists(), "the double marker is cleared");
    let _ = std::fs::remove_dir_all(root);
}
