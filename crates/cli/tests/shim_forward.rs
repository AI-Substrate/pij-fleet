//! Plan 213: the `pij` shim (`harness/scripts/pij-cli.cjs`) hands pij-rs's
//! community verbs to pij-rs with argv and stdin unchanged, and prints pij-rs's
//! own stdout, stderr and exit code.
//!
//! The verbs come from the shim's list, which `main.rs`'s
//! `shim_forward_tests` holds equal to clap's verbs that own their help. Each
//! verb's flags come from pij-rs's own help for it. Help through the shim is
//! compared byte for byte with the real binary; every other call goes to a
//! recording `pij-rs`, so no daemon is needed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use std::io::Write as _;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn native_verbs() -> Vec<String> {
    let file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("harness/scripts/pij-native-verbs.json"))
            .expect("read pij-native-verbs.json"),
    )
    .expect("pij-native-verbs.json is JSON");
    serde_json::from_value(file["verbs"].clone()).expect("a verbs array")
}

/// A scratch dir holding a recording `pij-rs`: it writes its argv
/// (NUL-separated) and stdin, prints a marker on each stream, and exits 7.
struct Recorder {
    dir: PathBuf,
}

impl Recorder {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pij-shim-forward-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let fake = dir.join("bin/pij-rs");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > '{0}/argv'\ncat > '{0}/stdin'\nprintf 'native out'\nprintf 'native err' >&2\nexit 7\n",
                dir.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        Self { dir }
    }

    fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// The argv the fake pij-rs saw, or `None` when the shim never ran it.
    fn take(&self) -> Option<(Vec<String>, Vec<u8>)> {
        let argv = std::fs::read(self.dir.join("argv")).ok()?;
        let stdin = std::fs::read(self.dir.join("stdin")).unwrap_or_default();
        let _ = std::fs::remove_file(self.dir.join("argv"));
        let _ = std::fs::remove_file(self.dir.join("stdin"));
        let argv = argv
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
            .collect();
        Some((argv, stdin))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run the shim with `bin` first on PATH, a neutral identity and generation,
/// and `env` on top.
fn shim(bin: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> Output {
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("node");
    command
        .arg(root().join("harness/scripts/pij-cli.cjs"))
        .args(args)
        .env("PATH", path)
        .env(
            "PIJ_RS_STATE_DIR",
            std::env::temp_dir().join("pij-shim-forward-no-daemon"),
        )
        .env("PIJ_RS_ADDR", "127.0.0.1:9")
        .env_remove("PIJ_DAEMON_GENERATION")
        .env_remove("PIJ_SESSION_ID")
        .env_remove("TMUX_PANE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .expect("run node harness/scripts/pij-cli.cjs");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// Whether `args` reach pij-rs exactly as typed, with stdin and pij-rs's own
/// output and exit code. `Err` says what differed.
fn reaches(recorder: &Recorder, args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    let stdin = b"line one\nline two\n";
    let output = shim(&recorder.bin(), args, env, stdin);
    let Some((argv, seen)) = recorder.take() else {
        return Err(format!("{args:?} never reached pij-rs"));
    };
    let expected: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    if argv != expected {
        return Err(format!("{args:?} reached pij-rs as {argv:?}"));
    }
    if seen != stdin {
        return Err(format!("{args:?}: stdin reached pij-rs as {seen:?}"));
    }
    if (
        output.stdout.as_slice(),
        output.stderr.as_slice(),
        output.status.code(),
    ) != (b"native out".as_slice(), b"native err".as_slice(), Some(7))
    {
        return Err(format!(
            "{args:?}: the shim did not print pij-rs's own output: {output:?}"
        ));
    }
    Ok(())
}

/// Every `--flag` pij-rs's help for `verb` names.
fn flags(verb: &str) -> BTreeSet<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([verb, "--help"])
        .env(
            "PIJ_RS_STATE_DIR",
            std::env::temp_dir().join("pij-shim-forward-no-daemon"),
        )
        .output()
        .expect("run pij-rs --help");
    let help = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    help.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| {
            word.starts_with("--") && word.len() > 2 && word.as_bytes()[2].is_ascii_lowercase()
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn help_through_the_shim_is_pij_rs_help() {
    let real = Path::new(env!("CARGO_BIN_EXE_pij-rs"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut verbs = native_verbs();
    verbs.push("send".to_string());
    for verb in &verbs {
        let native = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
            .args([verb.as_str(), "--help"])
            .env(
                "PIJ_RS_STATE_DIR",
                std::env::temp_dir().join("pij-shim-forward-no-daemon"),
            )
            .env("PIJ_RS_ADDR", "127.0.0.1:9")
            .output()
            .unwrap();
        let through = shim(&real, &[verb, "--help"], &[], b"");
        assert_eq!(
            (through.stdout, through.stderr, through.status.code()),
            (native.stdout, native.stderr, native.status.code()),
            "pij {verb} --help differs from pij-rs {verb} --help"
        );
    }
}

#[test]
fn every_community_verb_and_flag_reaches_pij_rs_unchanged() {
    let recorder = Recorder::new("verbs");
    for verb in native_verbs() {
        let flags = flags(&verb);
        assert!(!flags.is_empty(), "pij-rs {verb} --help names no flags");
        let mut args = vec![verb.as_str(), "probe", "two words", "--json"];
        for flag in &flags {
            args.extend([flag.as_str(), "value with spaces"]);
        }
        args.extend(["--body-file", "-"]);
        reaches(&recorder, &args, &[]).unwrap();
    }
}

#[test]
fn role_sends_and_the_native_send_form_reach_pij_rs_unchanged() {
    let recorder = Recorder::new("send");
    let pane = [("TMUX_PANE", "%9")];
    let session = [("PIJ_SESSION_ID", "pij-probe")];
    let send_flags = flags("send");
    for flag in ["--reply", "--limit", "--project", "--fyi", "--to", "--body"] {
        assert!(send_flags.contains(flag), "pij-rs send --help lacks {flag}");
    }
    for (args, env) in [
        (
            &["send", "role:security/141", "hello there", "--reply", "fyi"][..],
            &pane[..],
        ),
        (&["send", "--fyi", "role:pa", "held"], &pane),
        (
            &["send", "--limit", "45m", "role:qa", "--body-file", "-"],
            &session,
        ),
        (&["send", "--to", "role:qa", "--body", "x"], &pane),
        (
            &["send", "--to=pij-peer", "--body", "x", "--json"],
            &session,
        ),
        (&["send", "--help"], &[]),
        (&["send", "-h"], &[]),
    ] {
        reaches(&recorder, args, env).unwrap();
    }
}

/// Negative controls: the check fails for calls the shim keeps on its routed
/// path, so a passing run means the forwarded calls really were forwarded.
#[test]
fn calls_outside_the_contract_keep_the_routed_path() {
    let recorder = Recorder::new("kept");
    let pane = [("TMUX_PANE", "%9")];
    for (args, env, why) in [
        (
            &["spine", "events"][..],
            &pane[..],
            "a verb pij-rs does not own help for",
        ),
        (&["send", "pij-peer", "hello"], &pane, "a send to a seat"),
        (
            &["send", "role:pa", "hello"],
            &[],
            "a paneless role send needs registration first",
        ),
        (
            &["send", "--reason", "role:fake", "pij-peer", "x"],
            &pane,
            "a flag value is not the recipient",
        ),
        (
            &["directory"],
            &[("PIJ_DAEMON_GENERATION", "legacy")],
            "an explicit non-rs generation",
        ),
    ] {
        let reached = reaches(&recorder, args, env);
        assert!(
            reached
                .as_ref()
                .is_err_and(|why| why.contains("never reached")),
            "{why}: {args:?} -> {reached:?}"
        );
    }
}

#[test]
fn a_missing_pij_rs_is_named() {
    let node = Command::new("sh")
        .args(["-c", "command -v node"])
        .output()
        .unwrap();
    let node = PathBuf::from(String::from_utf8(node.stdout).unwrap().trim());
    let output = Command::new(node)
        .arg(root().join("harness/scripts/pij-cli.cjs"))
        .arg("claims")
        .env("PATH", "/nonexistent")
        .env_remove("PIJ_DAEMON_GENERATION")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(127), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pij-rs is not on PATH"),
        "{output:?}"
    );
}
