//! The build stamps reach the compiler and nothing else (#175 review, P175-F001).
//!
//! `just rs-install` and `just bounce-rs` stamp the daemon with its commit and
//! the extension source trees (`PIJ_BUILD_COMMIT`, `PIJ_EXT_TREE_COPILOT`,
//! `PIJ_EXT_TREE_OMP`). `bounce-rs` then launches the daemon, which passes its
//! environment to every background job, cargo in other worktrees included; an
//! exported stamp would compile those with production's identity. This runs
//! each recipe's own body with fakes for every external command (no git, no
//! compiler, no daemon, no bounce) and reads the environment each one saw.
//!
//! The same harness proves the daemon environment the fleet recipes pass: the
//! idle-holder nudge is off in a bare daemon and on from `bounce-rs` and the
//! LaunchAgent `rs-autostart` writes, unless `PIJ_IDLE_NUDGE` says otherwise
//! when they run (F147).
use std::path::{Path, PathBuf};
use std::process::Command;

const STAMPS: [&str; 3] = [
    "PIJ_BUILD_COMMIT",
    "PIJ_EXT_TREE_COPILOT",
    "PIJ_EXT_TREE_OMP",
];

/// The body of `recipe` in the justfile, unindented, without its shebang.
fn recipe(justfile: &str, recipe: &str) -> String {
    let mut lines = justfile
        .lines()
        .skip_while(|line| *line != format!("{recipe}:"))
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("    "))
        .map(|line| line.strip_prefix("    ").unwrap_or(line))
        .collect::<Vec<_>>();
    assert!(!lines.is_empty(), "the justfile has a `{recipe}` recipe");
    if lines[0].starts_with("#!") {
        lines.remove(0);
    }
    lines.join("\n")
}

fn fake(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write a fake");
    let mut permissions = std::fs::metadata(&path).expect("fake").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).expect("make the fake executable");
}

/// The stamps among one recorded environment.
fn stamps(env: &Path) -> Vec<String> {
    std::fs::read_to_string(env)
        .unwrap_or_else(|_| panic!("{} was never written", env.display()))
        .lines()
        .filter(|line| {
            STAMPS
                .iter()
                .any(|stamp| line.starts_with(&format!("{stamp}=")))
        })
        .map(str::to_string)
        .collect()
}

fn run(name: &str) -> PathBuf {
    run_with(name, &[])
}

/// [`run`], with `env` set for the recipe (as if typed before `just`).
fn run_with(name: &str, env: &[(&str, &str)]) -> PathBuf {
    let (scratch, ok) = try_run(name, env);
    assert!(ok, "`{name}` ran to the end with fakes");
    scratch
}

/// [`run_with`], returning whether the recipe succeeded instead of asserting.
fn try_run(name: &str, env: &[(&str, &str)]) -> (PathBuf, bool) {
    try_run_seeded(name, env, &[])
}

/// [`try_run`], with `seed` files (path under the scratch home, contents)
/// written first, as if a previous run had left them.
fn try_run_seeded(name: &str, env: &[(&str, &str)], seed: &[(&str, &str)]) -> (PathBuf, bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let justfile = std::fs::read_to_string(root.join("justfile")).expect("read the justfile");
    // One scratch per call: tests of the same recipe run in parallel.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "pij-stamps-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    let bin = scratch.join("bin");
    let global = scratch.join("global");
    std::fs::create_dir_all(&bin).expect("scratch bin");
    std::fs::create_dir_all(global.join("bin")).expect("scratch global bin");
    let out = scratch.display();
    fake(
        &bin,
        "git",
        "for id in 1111111111aaaa 2222222222bbbb 3333333333cccc 4444444444dddd; do echo $id; done",
    );
    fake(
        &bin,
        "cargo",
        &format!("env > '{out}/cargo.env'; echo Finished"),
    );
    fake(
        &bin,
        "npm",
        &format!("[ \"$1\" = prefix ] && echo '{}'; exit 0", global.display()),
    );
    // Nothing listens before the bounce; the new daemon after it.
    fake(
        &bin,
        "lsof",
        &format!("[ -f '{out}/lsof' ] && echo 4242; touch '{out}/lsof'; exit 0"),
    );
    fake(&bin, "sleep", "exit 0");
    fake(&bin, "plutil", "exit 0");
    fake(&bin, "launchctl", "exit 0");
    fake(
        &global.join("bin"),
        "pij-rs",
        &format!(
            "[ \"$1\" = daemon ] && env > '{out}/daemon.tmp' && mv '{out}/daemon.tmp' '{out}/daemon.env'; exit 0"
        ),
    );
    for (path, contents) in seed {
        let path = scratch.join(path);
        std::fs::create_dir_all(path.parent().expect("a seeded file's folder"))
            .expect("seed folder");
        std::fs::write(path, contents).expect("seed a file");
    }
    std::fs::write(scratch.join("recipe.sh"), recipe(&justfile, name)).expect("write the recipe");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let status = Command::new("/bin/sh")
        .arg(scratch.join("recipe.sh"))
        .current_dir(&scratch)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &scratch)
        .envs(env.iter().copied())
        .status()
        .expect("run the recipe");
    (scratch, status.success())
}

#[test]
fn bounce_rs_stamps_cargo_and_never_the_daemon() {
    let scratch = run("bounce-rs");
    let mut seen = stamps(&scratch.join("cargo.env"));
    seen.sort();
    assert_eq!(
        seen,
        [
            "PIJ_BUILD_COMMIT=1111111111",
            "PIJ_EXT_TREE_COPILOT=2222222222",
            "PIJ_EXT_TREE_OMP=3333333333+4444444444",
        ],
        "cargo sees every stamp"
    );
    // The daemon starts in the background; give it a moment to record.
    let daemon = scratch.join("daemon.env");
    for _ in 0..100 {
        if daemon.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        stamps(&daemon),
        Vec::<String>::new(),
        "the daemon, and so every job it runs, inherits no stamp"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn rs_install_stamps_cargo_and_exports_nothing() {
    let scratch = run("rs-install");
    let mut seen = stamps(&scratch.join("cargo.env"));
    seen.sort();
    assert_eq!(
        seen,
        [
            "PIJ_BUILD_COMMIT=1111111111",
            "PIJ_EXT_TREE_COPILOT=2222222222",
            "PIJ_EXT_TREE_OMP=3333333333+4444444444",
        ]
    );
    let justfile =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../justfile"))
            .expect("read the justfile");
    for stamp in STAMPS {
        assert!(
            !justfile.contains(&format!("export {stamp}")),
            "{stamp} is stamped on the cargo command, never exported"
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The environment the daemon `bounce-rs` launched was given.
fn daemon_env(scratch: &Path) -> String {
    let daemon = scratch.join("daemon.env");
    for _ in 0..100 {
        if daemon.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::fs::read_to_string(&daemon).expect("the daemon recorded its environment")
}

/// F147: `bounce-rs` starts the daemon with the idle nudge on, and
/// `PIJ_IDLE_NUDGE=0 just bounce-rs` turns it off for that bounce.
#[test]
fn bounce_rs_turns_the_idle_nudge_on_unless_told_otherwise() {
    for (env, expected) in [
        (&[][..], "PIJ_IDLE_NUDGE=1"),
        (&[("PIJ_IDLE_NUDGE", "0")][..], "PIJ_IDLE_NUDGE=0"),
    ] {
        let scratch = run_with("bounce-rs", env);
        let seen = daemon_env(&scratch);
        assert!(
            seen.lines().any(|line| line == expected),
            "{env:?}: the daemon sees {expected}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// F147: the LaunchAgent `rs-autostart` writes starts the daemon at login
/// with the same setting.
#[test]
fn rs_autostart_writes_the_idle_nudge_into_the_launch_agent() {
    for (env, expected) in [
        (&[][..], "<key>PIJ_IDLE_NUDGE</key><string>1</string>"),
        (
            &[("PIJ_IDLE_NUDGE", "0")][..],
            "<key>PIJ_IDLE_NUDGE</key><string>0</string>",
        ),
    ] {
        let scratch = run_with("rs-autostart", env);
        let plist =
            std::fs::read_to_string(scratch.join("Library/LaunchAgents/dev.pij.rs-daemon.plist"))
                .expect("rs-autostart wrote its plist");
        assert!(plist.contains(expected), "{env:?}: {plist}");
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// A value that would close the plist's string and dict and add a top-level
/// `Program` key, if it were written raw (#196 S196-001).
const HOSTILE: &str =
    "1</string></dict><key>Program</key><string>/tmp/evil</string><dict><key>x</key><string>";

const PLIST: &str = "Library/LaunchAgents/dev.pij.rs-daemon.plist";

/// The written plist parsed for real (Python's `plistlib`, on macOS and the
/// Linux CI alike; the recipe's own `plutil` is a fake here): its
/// `PIJ_IDLE_NUDGE` and whether it has a top-level `Program` key.
fn parsed(plist: &Path) -> String {
    let out = Command::new("python3")
        .args([
            "-c",
            "import plistlib, sys\nd = plistlib.load(open(sys.argv[1], 'rb'))\nprint(d['EnvironmentVariables']['PIJ_IDLE_NUDGE'], 'Program' in d)",
        ])
        .arg(plist)
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "the plist parses: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

/// S196-001: `PIJ_IDLE_NUDGE` reaches the LaunchAgent only as `1` or `0`.
/// Each accepted spelling is written as one of them; anything else is refused
/// and no plist is written.
#[test]
fn rs_autostart_writes_only_a_normalized_idle_nudge() {
    for (value, written) in [
        ("1", "1"),
        ("on", "1"),
        ("TRUE", "1"),
        ("True", "1"),
        ("0", "0"),
        ("off", "0"),
        ("FALSE", "0"),
    ] {
        let scratch = run_with("rs-autostart", &[("PIJ_IDLE_NUDGE", value)]);
        assert_eq!(
            parsed(&scratch.join(PLIST)),
            format!("{written} False"),
            "{value}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
    // A hostile or merely malformed value (`&` or `<` would make the plist
    // invalid) is refused before anything is written: no new plist, and an
    // installed one is left as it was (review of #196, S196-001 and F001).
    for value in [HOSTILE, "a&b", "<x>", "yes", ""] {
        let (scratch, ok) = try_run("rs-autostart", &[("PIJ_IDLE_NUDGE", value)]);
        assert!(!ok, "{value:?} is refused");
        assert!(
            !scratch.join(PLIST).exists(),
            "{value:?}: no plist is written"
        );
        let _ = std::fs::remove_dir_all(&scratch);
        let (scratch, ok) = try_run_seeded(
            "rs-autostart",
            &[("PIJ_IDLE_NUDGE", value)],
            &[(PLIST, "the plist already installed")],
        );
        assert!(!ok, "{value:?} is refused");
        assert_eq!(
            std::fs::read_to_string(scratch.join(PLIST)).expect("the old plist"),
            "the plist already installed",
            "{value:?}: the installed plist is untouched"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// S196-001, the same in `bounce-rs`: an accepted spelling reaches the daemon
/// as `1` or `0`; anything else is refused before anything is built or
/// launched.
#[test]
fn bounce_rs_passes_only_a_normalized_idle_nudge() {
    let scratch = run_with("bounce-rs", &[("PIJ_IDLE_NUDGE", "OFF")]);
    assert!(
        daemon_env(&scratch)
            .lines()
            .any(|line| line == "PIJ_IDLE_NUDGE=0")
    );
    let _ = std::fs::remove_dir_all(&scratch);
    let (scratch, ok) = try_run("bounce-rs", &[("PIJ_IDLE_NUDGE", HOSTILE)]);
    assert!(!ok, "refused");
    assert!(!scratch.join("cargo.env").exists(), "nothing was built");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!scratch.join("daemon.env").exists(), "nothing was launched");
    let _ = std::fs::remove_dir_all(&scratch);
}
