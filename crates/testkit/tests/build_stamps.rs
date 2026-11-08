//! The build stamps reach the compiler and nothing else (#175 review, P175-F001).
//!
//! `just rs-install` and `just bounce-rs` stamp the daemon with its commit and
//! the extension source trees (`PIJ_BUILD_COMMIT`, `PIJ_EXT_TREE_COPILOT`,
//! `PIJ_EXT_TREE_OMP`). `bounce-rs` then launches the daemon, which passes its
//! environment to every background job, cargo in other worktrees included; an
//! exported stamp would compile those with production's identity. This runs
//! each recipe's own body with fakes for every external command (no git, no
//! compiler, no daemon, no bounce) and reads the environment each one saw.
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let justfile = std::fs::read_to_string(root.join("justfile")).expect("read the justfile");
    let scratch = std::env::temp_dir().join(format!("pij-stamps-{name}-{}", std::process::id()));
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
    fake(
        &global.join("bin"),
        "pij-rs",
        &format!(
            "[ \"$1\" = daemon ] && env > '{out}/daemon.tmp' && mv '{out}/daemon.tmp' '{out}/daemon.env'; exit 0"
        ),
    );
    std::fs::write(scratch.join("recipe.sh"), recipe(&justfile, name)).expect("write the recipe");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let status = Command::new("/bin/sh")
        .arg(scratch.join("recipe.sh"))
        .current_dir(&scratch)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &scratch)
        .status()
        .expect("run the recipe");
    assert!(status.success(), "`{name}` ran to the end with fakes");
    scratch
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
