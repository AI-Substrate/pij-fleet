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
//!
//! And where pij-rs is installed (F187): `rs-install` and `bounce-rs` put a
//! copy outside target/ and link PATH to it, `bounce-rs` launches that copy,
//! and the check `just doctor` runs flags any pij binary on PATH that resolves
//! into a target/.
use std::path::{Path, PathBuf};
use std::process::Command;

const STAMPS: [&str; 3] = [
    "PIJ_BUILD_COMMIT",
    "PIJ_EXT_TREE_COPILOT",
    "PIJ_EXT_TREE_OMP",
];

/// The body of `recipe` in the justfile, unindented, without its shebang, and
/// the shell it names (`bash` when its shebang says so, else `/bin/sh`).
fn recipe(justfile: &str, recipe: &str) -> (String, &'static str) {
    let mut lines = justfile
        .lines()
        .skip_while(|line| *line != format!("{recipe}:"))
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("    "))
        .map(|line| line.strip_prefix("    ").unwrap_or(line))
        .collect::<Vec<_>>();
    assert!(!lines.is_empty(), "the justfile has a `{recipe}` recipe");
    let mut shell = "/bin/sh";
    if lines[0].starts_with("#!") {
        if lines[0].ends_with("bash") {
            shell = "/bin/bash";
        }
        lines.remove(0);
    }
    (lines.join("\n"), shell)
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
    try_run_faked(name, env, seed, &[])
}

/// [`try_run_seeded`], with `fakes` (name, body) written over the default
/// fakes; `@OUT@` in a body, or in an `env` value, is the scratch home.
fn try_run_faked(
    name: &str,
    env: &[(&str, &str)],
    seed: &[(&str, &str)],
    fakes: &[(&str, &str)],
) -> (PathBuf, bool) {
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
    // The ids rev-parse names, the scratch as the checkout and a clean status
    // (F226: the deploy build resolves its target through git and refuses dirty
    // build inputs).
    fake(
        &bin,
        "git",
        &format!(
            "case \"$*\" in status*) ;; *--show-toplevel*) echo '{out}';; *--git-common-dir*) echo '{out}/.git';; \
             *) for id in 1111111111aaaa 2222222222bbbb 3333333333cccc 4444444444dddd; do echo $id; done;; esac"
        ),
    );
    // The build: a pij-rs stamped with the commit the fake git names, which
    // records the environment it runs the daemon with.
    fake(
        &scratch,
        "artifact",
        &format!(
            "[ \"$1\" = --version ] && echo 'pij-rs 0.1.0 (1111111111)' && exit 0\n\
             [ \"$1\" = daemon ] && env > '{out}/daemon.tmp' && mv '{out}/daemon.tmp' '{out}/daemon.env'; exit 0"
        ),
    );
    fake(
        &bin,
        "cargo",
        &format!(
            "env > '{out}/cargo.env'; mkdir -p \"$CARGO_TARGET_DIR/release\" && cp '{out}/artifact' \"$CARGO_TARGET_DIR/release/pij-rs\"; echo Finished"
        ),
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
    // An older install on PATH: launching it instead of the fresh build is a bug.
    fake(
        &global.join("bin"),
        "pij-rs",
        &format!("touch '{out}/stale-daemon.env'; exit 0"),
    );
    let scripts = scratch.join("harness/scripts");
    std::fs::create_dir_all(&scripts).expect("scratch scripts");
    // The scripts the recipes run (F220: they nice through nice19.sh; F226:
    // they build through release-build.sh, which runs shared-build).
    for script in [
        "install-pij-rs.sh",
        "nice19.sh",
        "release-build.sh",
        "shared-build.sh",
        "cargo-target.sh",
    ] {
        std::fs::copy(
            root.join("harness/scripts").join(script),
            scripts.join(script),
        )
        .expect("a script the recipes run");
    }
    for (path, contents) in seed {
        let path = scratch.join(path);
        std::fs::create_dir_all(path.parent().expect("a seeded file's folder"))
            .expect("seed folder");
        std::fs::write(path, contents).expect("seed a file");
    }
    for (name, body) in fakes {
        fake(&bin, name, &body.replace("@OUT@", &out.to_string()));
    }
    let (body, shell) = recipe(&justfile, name);
    std::fs::write(scratch.join("recipe.sh"), body).expect("write the recipe");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let status = Command::new(shell)
        .arg(scratch.join("recipe.sh"))
        .current_dir(&scratch)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &scratch)
        .envs(
            env.iter()
                .map(|(key, value)| (*key, value.replace("@OUT@", &out.to_string()))),
        )
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

/// The installed copy and the PATH entry linked to it, in a scratch home.
fn installed(scratch: &Path) -> (PathBuf, PathBuf) {
    (
        scratch.join(".local/share/pij/bin/pij-rs"),
        scratch.join("global/bin/pij-rs"),
    )
}

/// F187: `rs-install` links PATH to a copy outside target/, so deleting
/// target/ (a disk sweep did, and the CLI was down for 55 minutes) leaves
/// pij-rs running.
#[test]
fn rs_install_links_path_to_a_copy_that_outlives_target() {
    let scratch = run("rs-install");
    let (copy, link) = installed(&scratch);
    assert_eq!(std::fs::read_link(&link).expect("PATH links"), copy);
    assert!(
        std::fs::symlink_metadata(&copy)
            .expect("the copy")
            .file_type()
            .is_file(),
        "the copy is a file, not another link"
    );
    assert_eq!(
        std::fs::read(&copy).expect("the copy"),
        std::fs::read(scratch.join("target/release/pij-rs")).expect("the build")
    );
    let (ok, out) = path_check(&scratch, &[scratch.join("global/bin")]);
    assert!(ok, "the doctor check passes: {out}");
    std::fs::remove_dir_all(scratch.join("target")).expect("delete target/");
    assert!(
        Command::new(&link).status().expect("run pij-rs").success(),
        "pij-rs still runs"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// F187: `bounce-rs` installs its fresh build the same way and launches that
/// copy, never an older install or target/.
#[test]
fn bounce_rs_launches_its_fresh_copy() {
    let scratch = run("bounce-rs");
    daemon_env(&scratch);
    assert!(
        !scratch.join("stale-daemon.env").exists(),
        "the older install never ran"
    );
    let (copy, link) = installed(&scratch);
    assert_eq!(std::fs::read_link(&link).expect("PATH links"), copy);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `_pij-bins-path-check`, which `just doctor` runs, over `dirs` then
/// `/usr/bin:/bin`: whether it passed, and what it printed.
fn path_check(scratch: &Path, dirs: &[PathBuf]) -> (bool, String) {
    let justfile =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../justfile"))
            .expect("read the justfile");
    let script = scratch.join("path-check.sh");
    let (body, shell) = recipe(&justfile, "_pij-bins-path-check");
    std::fs::write(&script, body).expect("write the check");
    let mut path = dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>();
    path.extend(["/usr/bin".to_string(), "/bin".to_string()]);
    let output = Command::new(shell)
        .arg(&script)
        .env_clear()
        .env("PATH", path.join(":"))
        .output()
        .expect("run the check");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// F187: doctor flags the pij binary on PATH that runs when it resolves into a
/// target/, or a broken link when nothing of that name runs, with its fix; the
/// installed copy is clean.
#[test]
fn doctor_flags_a_pij_binary_that_resolves_into_target() {
    let scratch = std::env::temp_dir().join(format!("pij-path-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let release = scratch.join("repo/target/release");
    let copies = scratch.join("home/.local/share/pij/bin");
    for dir in [&release, &copies] {
        std::fs::create_dir_all(dir).expect("scratch dir");
    }
    fake(&release, "pij-rs", "exit 0");
    fake(&release, "pij-gate", "exit 0");
    fake(&copies, "pij-rs", "exit 0");
    let linked = |name: &str, to: &Path| {
        let dir = scratch.join(name);
        std::fs::create_dir_all(&dir).expect("PATH dir");
        let file = to.file_name().expect("a binary name");
        std::os::unix::fs::symlink(to, dir.join(file)).expect("link");
        dir
    };
    let into_target = linked("into-target", &release.join("pij-rs"));
    let gate = linked("gate", &release.join("pij-gate"));
    let broken = linked("broken", &scratch.join("repo/gone/pij-rs"));
    let copy = linked("copy", &copies.join("pij-rs"));

    let (ok, out) = path_check(&scratch, std::slice::from_ref(&into_target));
    assert!(!ok, "a link into target/ is flagged");
    assert!(
        out.contains("resolves into a cargo target/") && out.contains("run: just rs-install"),
        "{out}"
    );
    let (ok, out) = path_check(&scratch, std::slice::from_ref(&gate));
    assert!(!ok && out.contains("pij-gate"), "any pij binary: {out}");
    let (ok, out) = path_check(&scratch, std::slice::from_ref(&broken));
    assert!(
        !ok && out.contains("broken link") && out.contains("run: just rs-install"),
        "{out}"
    );
    let (ok, out) = path_check(&scratch, std::slice::from_ref(&copy));
    assert!(ok, "the installed copy is clean: {out}");
    // Only the first of each name on PATH runs, so only it is judged.
    let (ok, out) = path_check(&scratch, &[copy.clone(), into_target.clone()]);
    assert!(ok, "a shadowed link is not what runs: {out}");
    let (ok, out) = path_check(&scratch, &[broken, copy]);
    assert!(ok, "a broken link before one that runs: {out}");
    // What runs is the first executable file, as the shim resolves pij-rs
    // (#229 review): a file that isn't executable hides nothing.
    let unrunnable = scratch.join("unrunnable");
    std::fs::create_dir_all(&unrunnable).expect("PATH dir");
    std::fs::write(unrunnable.join("pij-rs"), "not a program").expect("a plain file");
    let (ok, out) = path_check(&scratch, &[unrunnable, into_target]);
    assert!(
        !ok && out.contains("resolves into a cargo target/"),
        "the link into target/ is what runs: {out}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The old daemon `bounce-rs` stops: a real process (its pid is what the fake
/// `lsof` names first), reaped as soon as it ends, so a stopped one is gone.
struct OldDaemon {
    pid: u32,
    ended: std::sync::mpsc::Receiver<()>,
}

impl OldDaemon {
    fn start() -> Self {
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("an old daemon");
        let pid = child.id();
        let (tx, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
        });
        Self { pid, ended }
    }

    /// Whether it ended. `bounce-rs` waits for its pid to go before it moves
    /// on, so once the recipe returns only the reap can lag.
    fn stopped(&self) -> bool {
        self.ended
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
    }
}

impl Drop for OldDaemon {
    fn drop(&mut self) {
        let _ = Command::new("/bin/kill")
            .args(["-9", &self.pid.to_string()])
            .status();
    }
}

/// `bounce-rs` against a running old daemon with `tmux` faked by `tmux` (none
/// faked: `None`): the scratch, and whether it succeeded. `lsof` on PATH names
/// the old daemon, then a free port, then the new daemon.
fn bounce_with_tmux(old: &OldDaemon, tmux: Option<&str>, env: &[(&str, &str)]) -> (PathBuf, bool) {
    // F245: who holds a refused socket is asked of `/usr/sbin/lsof` by its
    // absolute path (here the `PIJ_BOUNCE_LSOF` seam), never PATH's `lsof`,
    // which fails that question. It answers `$FAKE_HELD`, fails with
    // `$FAKE_LSOF_U_FAILS`, or never finishes with `$FAKE_LSOF_U_HANGS`.
    let lsof = format!(
        "case \"$*\" in *-U*) exit 3 ;; esac\n\
         n=$(cat '@OUT@/lsof.n' 2>/dev/null || echo 0); n=$((n + 1)); echo $n > '@OUT@/lsof.n'\n\
         case $n in 1) echo {} ;; 2) ;; *) echo 4242 ;; esac; exit 0",
        old.pid
    );
    let socket_lsof = "echo \"$*\" > '@OUT@/socket-lsof.args'\n\
                       [ -n \"$FAKE_LSOF_U_HANGS\" ] && exec /bin/sleep 60\n\
                       [ -n \"$FAKE_LSOF_U_FAILS\" ] && exit 1\n\
                       printf '%s\\n' \"$FAKE_HELD\"";
    // F247: the uid comes from bash, never a launched `id`.
    let id = "echo \"$*\" >> '@OUT@/id.called'; exec /usr/bin/id \"$@\"";
    let mut fakes = vec![
        ("lsof", lsof.as_str()),
        ("socket-lsof", socket_lsof),
        ("id", id),
    ];
    fakes.extend(tmux.map(|body| ("tmux", body)));
    let mut env = env.to_vec();
    env.push(("PIJ_BOUNCE_LSOF", "@OUT@/bin/socket-lsof"));
    try_run_faked("bounce-rs", &env, &[], &fakes)
}

/// F242: before it signals the old daemon, `bounce-rs` switches `_pij`'s
/// destroy-unattached off on the daemon's server, then stops it. The server
/// is named as the daemon names it, a `-L` label or none, and tmux finds the
/// socket itself, with its own check of the socket directory: never `-S`
/// (S293-001).
#[test]
fn bounce_rs_switches_destroy_unattached_off_before_the_signal() {
    for (env, expected) in [
        (&[][..], "set-option -t _pij: destroy-unattached off"),
        (
            &[("PIJ_TMUX_SERVER", "fleet")][..],
            "-L fleet set-option -t _pij: destroy-unattached off",
        ),
    ] {
        let old = OldDaemon::start();
        let tmux = format!(
            "kill -0 {} && echo \"$*\" > '@OUT@/tmux.args'; exit 0",
            old.pid
        );
        let (scratch, ok) = bounce_with_tmux(&old, Some(&tmux), env);
        assert!(ok, "{env:?}: the bounce goes on");
        let args = std::fs::read_to_string(scratch.join("tmux.args"))
            .expect("tmux ran while the old daemon was still running");
        assert_eq!(args.trim(), expected, "{env:?}");
        assert!(old.stopped(), "{env:?}: then the old daemon is stopped");
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// A tmux whose connect is refused, as tmux says it: its socket is a real
/// file at `@OUT@/real/$FAKE_SOCK_NAME` (default `default`), `@OUT@/alias`
/// is another spelling of `@OUT@/real` (bee's `/tmp` and `/private/tmp`),
/// `@OUT@/real/a^Ab` is a file named with a caret, `@OUT@/real/o<tab>x` one
/// with a tab, and `@OUT@/shut` is a
/// directory nobody can search. `$FAKE_NO_SOCK` leaves the socket out.
const REFUSED_TMUX: &str = "mkdir -p '@OUT@/real' '@OUT@/shut' && chmod 000 '@OUT@/shut'\n\
     ln -sfn '@OUT@/real' '@OUT@/alias'\n\
     : > '@OUT@/real/a^Ab'; : > \"@OUT@/real/o$(printf '\\t')x\"\n\
     sock=\"@OUT@/real/${FAKE_SOCK_NAME:-default}\"; [ -n \"$FAKE_NO_SOCK\" ] || : > \"$sock\"\n\
     printf 'no server running on %s\\n' \"$sock\" >&2; exit 1";

fn unshut(scratch: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(scratch.join("shut"), std::fs::Permissions::from_mode(0o700));
}

/// F242: no server, or no `_pij` (a first boot), has nothing to protect.
#[test]
fn bounce_rs_goes_on_when_tmux_has_nothing_to_protect() {
    // A refused socket no process holds (`lsof -U` names others only).
    let others = [("FAKE_HELD", "n@OUT@/real")];
    for said in [
        None,
        Some("error connecting to /tmp/tmux-1/default (No such file or directory)"),
        Some("no such session: _pij:"),
    ] {
        let old = OldDaemon::start();
        let tmux = said.map_or_else(
            || REFUSED_TMUX.to_string(),
            |said| format!("echo '{said}' >&2; exit 1"),
        );
        let (scratch, ok) = bounce_with_tmux(&old, Some(&tmux), &others);
        assert!(ok, "{said:?}: the bounce goes on");
        assert!(old.stopped(), "{said:?}: the old daemon is stopped");
        unshut(&scratch);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// F245: tmux says `no server running on <socket>` when its connect is
/// refused, and on Darwin a live server with a full listen backlog refuses
/// too. The bounce goes on only when no process holds that socket; one that
/// is held, or holders that cannot be read, refuse it, unless
/// `PIJ_BOUNCE_UNSAFE=1`. #298 round 2: a holder is the socket by device and
/// inode whatever its spelling (bee's alias), lsof's escapes are decoded and
/// a name it can't spell exactly refuses (sawfish), and so does any bound
/// path that can't be looked at.
#[test]
fn bounce_rs_goes_on_after_a_refusal_only_when_nothing_holds_the_socket() {
    for (env, refused_bounce) in [
        (vec![("FAKE_HELD", "n@OUT@/real/default")], true),
        (vec![("FAKE_HELD", "n@OUT@/alias/default")], true),
        (
            vec![
                ("FAKE_SOCK_NAME", "t\tb"),
                ("FAKE_HELD", r"n@OUT@/alias/t\tb"),
            ],
            true,
        ),
        (
            vec![
                ("FAKE_SOCK_NAME", r"back\b"),
                ("FAKE_HELD", r"n@OUT@/real/back\\b"),
            ],
            true,
        ),
        (vec![("FAKE_HELD", "n@OUT@/real/a^Ab")], true),
        (vec![("FAKE_HELD", "n@OUT@/real/unlinked")], true),
        (vec![("FAKE_HELD", "n@OUT@/shut/sock")], true),
        (vec![("FAKE_LSOF_U_FAILS", "1")], true),
        (
            vec![("FAKE_NO_SOCK", "1"), ("FAKE_HELD", "n@OUT@/real")],
            true,
        ),
        (vec![("FAKE_HELD", "n@OUT@/real\nn->0x1")], false),
        // Another socket, named with a tab lsof escapes: decoded, it is there
        // and is not this socket.
        (vec![("FAKE_HELD", r"n@OUT@/real/o\tx")], false),
        (
            vec![
                ("FAKE_HELD", "n@OUT@/alias/default"),
                ("PIJ_BOUNCE_UNSAFE", "1"),
            ],
            false,
        ),
    ] {
        let old = OldDaemon::start();
        let (scratch, ok) = bounce_with_tmux(&old, Some(REFUSED_TMUX), &env);
        assert_eq!(!ok, refused_bounce, "{env:?}: refused");
        assert_eq!(
            old.stopped(),
            !refused_bounce,
            "{env:?}: the old daemon stopped"
        );
        unshut(&scratch);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// S299-N2 (deer): macOS's bash 3.2 takes UID from the environment. A UID
/// that isn't all digits refuses the bounce (unless `PIJ_BOUNCE_UNSAFE=1`)
/// rather than reach lsof. A bash that ignores an inherited UID (bash 5,
/// where it is read-only) asks lsof for the real uid instead.
#[test]
fn bounce_rs_refuses_when_uid_is_not_a_number() {
    let imports = std::process::Command::new("/usr/bin/env")
        .args(["bash", "-c", "printf %s \"$UID\""])
        .env("UID", "^501")
        .output()
        .map(|out| out.stdout == b"^501")
        .expect("run bash");
    for (unsafe_ok, refused) in [(false, imports), (true, false)] {
        let old = OldDaemon::start();
        let mut env = vec![("UID", "^501"), ("FAKE_HELD", "n@OUT@/real")];
        if unsafe_ok {
            env.push(("PIJ_BOUNCE_UNSAFE", "1"));
        }
        let (scratch, ok) = bounce_with_tmux(&old, Some(REFUSED_TMUX), &env);
        assert_eq!(
            !ok, refused,
            "bash imports UID: {imports}; UNSAFE: {unsafe_ok}"
        );
        assert_eq!(old.stopped(), !refused, "the old daemon stopped");
        let args = std::fs::read_to_string(scratch.join("socket-lsof.args")).ok();
        if imports {
            assert_eq!(args, None, "lsof was asked with a UID that isn't a number");
        } else {
            let uid = pij_core::program::real_uid();
            let args = args.expect("lsof ran");
            assert!(args.contains(&format!("-u {uid} ")), "{args}");
        }
        unshut(&scratch);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// F247 (snail, P298-R2-001): the holder read names this user by bash's own
/// `$UID`, and launches no `id` for it: a bounce that reads the holders
/// launches as many `id`s as one that doesn't (the install step has its own).
#[test]
fn bounce_rs_names_the_user_to_lsof_without_launching_id() {
    let ids = |tmux: &str| {
        let old = OldDaemon::start();
        let (scratch, ok) = bounce_with_tmux(&old, Some(tmux), &[("FAKE_HELD", "n@OUT@/real")]);
        assert!(ok, "{tmux}: the bounce goes on");
        let args = std::fs::read_to_string(scratch.join("socket-lsof.args")).ok();
        let ids = std::fs::read_to_string(scratch.join("id.called")).unwrap_or_default();
        unshut(&scratch);
        let _ = std::fs::remove_dir_all(&scratch);
        (args, ids.lines().count())
    };
    let (read, with_lsof) = ids(REFUSED_TMUX);
    let (none, without) = ids("echo 'no such session: _pij:' >&2; exit 1");
    let uid = pij_core::program::real_uid();
    let read = read.expect("the refusal read the holders");
    assert!(read.contains(&format!("-u {uid} ")), "{read}");
    assert_eq!(none, None, "no refusal, no holder read");
    assert_eq!(with_lsof, without, "the holder read launched id");
}

/// F245 round 2 (snail): the holder read is bounded like the tmux call. An
/// `lsof` that never finishes is killed after 1 to 5 s and refuses the bounce
/// (fail closed), well before the fake's 60 s; `PIJ_BOUNCE_UNSAFE=1` goes on.
#[test]
fn bounce_rs_refuses_when_the_holder_read_does_not_finish() {
    let refused = "echo 'no server running on /tmp/tmux-1/default' >&2; exit 1";
    for (env, refused_bounce) in [
        (vec![("FAKE_LSOF_U_HANGS", "1")], true),
        (
            vec![("FAKE_LSOF_U_HANGS", "1"), ("PIJ_BOUNCE_UNSAFE", "1")],
            false,
        ),
    ] {
        let old = OldDaemon::start();
        let started = std::time::Instant::now();
        let (scratch, ok) = bounce_with_tmux(&old, Some(refused), &env);
        let took = started.elapsed();
        assert_eq!(!ok, refused_bounce, "{env:?}: refused");
        assert_eq!(
            old.stopped(),
            !refused_bounce,
            "{env:?}: the old daemon stopped"
        );
        assert!(
            took < pij_testkit::time::scaled(std::time::Duration::from_secs(20)),
            "{env:?}: bounded, took {took:?}"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// F242 (P293-001): a tmux that does not answer within the bound, or fails
/// any other way, refuses the bounce: the old daemon is left running.
/// `PIJ_BOUNCE_UNSAFE=1` (a human who checked the server) goes on.
#[test]
fn bounce_rs_refuses_when_destroy_unattached_cannot_be_switched_off() {
    for (tmux, env, refused) in [
        ("exec /bin/sleep 30", &[][..], true),
        (
            "echo 'error connecting to /tmp/tmux-1/default (Permission denied)' >&2; exit 1",
            &[][..],
            true,
        ),
        (
            "exec /bin/sleep 30",
            &[("PIJ_BOUNCE_UNSAFE", "1")][..],
            false,
        ),
    ] {
        let old = OldDaemon::start();
        let started = std::time::Instant::now();
        let (scratch, ok) = bounce_with_tmux(&old, Some(tmux), env);
        assert!(
            started.elapsed() < pij_testkit::time::scaled(std::time::Duration::from_secs(20)),
            "{tmux}: bounded"
        );
        assert_eq!(!ok, refused, "{tmux} {env:?}: refused");
        assert_eq!(
            old.stopped(),
            !refused,
            "{tmux} {env:?}: the old daemon stopped"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// A directory of links to every command in `/usr/bin` and `/bin` but `tmux`,
/// so a recipe run with it on PATH finds no tmux wherever CI installed one.
fn system_path_without_tmux() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let farm = std::env::temp_dir().join(format!(
        "pij-stamps-no-tmux-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&farm);
    std::fs::create_dir_all(&farm).expect("the farm");
    for dir in ["/usr/bin", "/bin"] {
        for entry in std::fs::read_dir(dir).expect("a system bin").flatten() {
            let name = entry.file_name();
            if name != "tmux" && !farm.join(&name).exists() {
                let _ = std::os::unix::fs::symlink(entry.path(), farm.join(&name));
            }
        }
    }
    farm
}

/// F244: no tmux on the caller's PATH (the daemon resolves tmux by the same
/// PATH search) leaves nothing to switch `_pij`'s destroy-unattached off
/// with: refused, never skipped. `PIJ_BOUNCE_UNSAFE=1` goes on.
#[test]
fn bounce_rs_refuses_when_tmux_is_not_on_path() {
    let farm = system_path_without_tmux();
    let path = format!("@OUT@/bin:{}", farm.display());
    for (env, refused) in [
        (vec![("PATH", path.as_str())], true),
        (
            vec![("PATH", path.as_str()), ("PIJ_BOUNCE_UNSAFE", "1")],
            false,
        ),
    ] {
        let old = OldDaemon::start();
        let (scratch, ok) = bounce_with_tmux(&old, None, &env);
        assert_eq!(!ok, refused, "{env:?}: refused");
        assert_eq!(old.stopped(), !refused, "{env:?}: the old daemon stopped");
        let _ = std::fs::remove_dir_all(&scratch);
    }
    let _ = std::fs::remove_dir_all(&farm);
}

/// F244 (P293-R2-001): a tmux that outlives its kill -9 (one stuck in the
/// kernel; here `kill -9` is made a no-op by an exported bash function) holds
/// the bounce only for the second bounded wait, and refuses it.
/// `PIJ_BOUNCE_UNSAFE=1` goes on.
#[test]
fn bounce_rs_refuses_when_tmux_outlives_its_kill() {
    let tmux = "echo $$ > '@OUT@/setter.pid'; exec /bin/sleep 30";
    let deaf = "() { [ \"$1\" = -9 ] && return 0; builtin kill \"$@\"; }";
    for (env, refused) in [
        (vec![("BASH_FUNC_kill%%", deaf)], true),
        (
            vec![("BASH_FUNC_kill%%", deaf), ("PIJ_BOUNCE_UNSAFE", "1")],
            false,
        ),
    ] {
        let old = OldDaemon::start();
        let started = std::time::Instant::now();
        let (scratch, ok) = bounce_with_tmux(&old, Some(tmux), &env);
        let took = started.elapsed();
        if let Ok(pid) = std::fs::read_to_string(scratch.join("setter.pid")) {
            let _ = Command::new("/bin/kill").args(["-9", pid.trim()]).status();
        }
        assert!(
            took < pij_testkit::time::scaled(std::time::Duration::from_secs(20)),
            "{env:?}: bounded, {took:?}"
        );
        assert_eq!(!ok, refused, "{env:?}: refused");
        assert_eq!(old.stopped(), !refused, "{env:?}: the old daemon stopped");
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
