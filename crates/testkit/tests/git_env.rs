//! Incident #354: a seat exported GIT_DIR/GIT_WORK_TREE and the tests' git
//! fixtures wrote into the real shared repository (core.worktree, a `pij test`
//! identity, branches `wt-branch` and `dup`).
//!
//! The test runs the incident's fixture operations through
//! [`pij_testkit::git_env`] in a child test process whose environment points
//! GIT_DIR and GIT_WORK_TREE at a decoy repository, then proves the decoy's
//! config, refs and worktrees are byte-identical. The child is this binary run
//! directly, not through cargo's runner, so the runner's refusal does not hide
//! the helper. A second test proves a fixture that sets its own identity is not
//! overridden by an inherited GIT_AUTHOR_*/GIT_COMMITTER_* (B357-R1-01).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pij_testkit::git_env;

const CHILD_SCRATCH: &str = "PIJ_GIT_ENV_TEST_SCRATCH";
const TEST_NAME: &str = "a_fixture_under_an_inherited_git_dir_leaves_the_decoy_untouched";
const IDENTITY_TEST: &str = "a_fixture_that_sets_its_own_identity_wins_over_an_inherited_one";

#[test]
fn a_fixture_under_an_inherited_git_dir_leaves_the_decoy_untouched() {
    if let Some(scratch) = std::env::var_os(CHILD_SCRATCH) {
        incident_fixture(Path::new(&scratch));
        return;
    }
    let root = pij_testkit::fresh_dir("pij-git-env-decoy")
        .canonicalize()
        .expect("canonical root");
    let decoy = root.join("decoy");
    let scratch = root.join("scratch");
    std::fs::create_dir_all(&decoy).expect("decoy dir");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=decoy",
            "-c",
            "user.email=decoy@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "decoy",
        ],
    ] {
        let mut command = git_env::git(&decoy);
        run(git_env::scrub_identity(&mut command).args(args));
    }
    let before = snapshot(&decoy.join(".git"));

    let child = run_child(
        TEST_NAME,
        &scratch,
        &[
            ("GIT_DIR", decoy.join(".git").as_os_str()),
            ("GIT_WORK_TREE", decoy.as_os_str()),
        ],
    );
    let after = snapshot(&decoy.join(".git"));
    let made_own_repo = scratch.join(".git/config").is_file();
    std::fs::remove_dir_all(&root).expect("remove the decoy root");

    assert!(
        child.status.success(),
        "the child fixture failed: {}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(
        changed(&before, &after),
        Vec::<String>::new(),
        "the decoy's config, refs or worktrees changed"
    );
    assert!(made_own_repo, "the fixture never built its own repository");
}

#[test]
fn a_fixture_that_sets_its_own_identity_wins_over_an_inherited_one() {
    if let Some(scratch) = std::env::var_os(CHILD_SCRATCH) {
        incident_fixture(Path::new(&scratch));
        return;
    }
    let root = pij_testkit::fresh_dir("pij-git-env-identity")
        .canonicalize()
        .expect("canonical root");
    let scratch = root.join("scratch");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let inherited = [
        ("GIT_AUTHOR_NAME", "inherited author"),
        ("GIT_AUTHOR_EMAIL", "author@inherited.invalid"),
        ("GIT_AUTHOR_DATE", "2001-02-03T04:05:06Z"),
        ("GIT_COMMITTER_NAME", "inherited committer"),
        ("GIT_COMMITTER_EMAIL", "committer@inherited.invalid"),
        ("GIT_COMMITTER_DATE", "2001-02-03T04:05:06Z"),
    ]
    .map(|(name, value)| (name, OsStr::new(value)));
    let child = run_child(IDENTITY_TEST, &scratch, &inherited);
    let identity = child.status.success().then(|| {
        let mut command = git_env::git(&scratch);
        let output = git_env::scrub_identity(&mut command)
            .args([
                "log",
                "-1",
                "--date=format:%Y",
                "--format=%an <%ae>|%cn <%ce>|%ad|%cd",
            ])
            .output()
            .expect("read the fixture's commit");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    });
    std::fs::remove_dir_all(&root).expect("remove the scratch root");

    assert!(
        child.status.success(),
        "the child fixture failed: {}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    let identity = identity.expect("the fixture's commit");
    let parts: Vec<&str> = identity.split('|').collect();
    assert_eq!(
        parts[..2],
        [
            "pij test <pij@example.invalid>",
            "pij test <pij@example.invalid>"
        ],
        "the inherited identity overrode the fixture's own: {identity}"
    );
    assert!(
        parts[2..].iter().all(|year| *year != "2001"),
        "the inherited author or committer date reached the fixture's commit: {identity}"
    );
}

/// This test binary, run directly as `test` in child mode with `env` added.
fn run_child(test: &str, scratch: &Path, env: &[(&str, &OsStr)]) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("this test binary"));
    command
        .args([test, "--exact", "--nocapture", "--test-threads", "1"])
        .env(CHILD_SCRATCH, scratch);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run the child test")
}

/// What the incident's fixtures did: init, a local identity, a commit, a
/// branch and a linked worktree.
fn incident_fixture(scratch: &Path) {
    let mut steps: Vec<Vec<String>> = [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "pij test"],
        &["config", "user.email", "pij@example.invalid"],
        &["commit", "-q", "--allow-empty", "-m", "fixture"],
        &["branch", "dup"],
    ]
    .iter()
    .map(|args| args.iter().map(|arg| (*arg).to_string()).collect())
    .collect();
    let linked = scratch.with_file_name("scratch-linked");
    steps.push(vec![
        "worktree".into(),
        "add".into(),
        "-q".into(),
        "-b".into(),
        "wt-branch".into(),
        linked.display().to_string(),
    ]);
    for args in &steps {
        let mut command = git_env::git(scratch);
        run(git_env::scrub_identity(&mut command).args(args));
    }
}

fn run(command: &mut Command) {
    let output = command.output().expect("run git");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every byte of the decoy's config, HEAD, refs, packed refs and worktrees.
fn snapshot(git_dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for name in ["config", "HEAD", "packed-refs", "refs", "worktrees"] {
        collect(git_dir, &git_dir.join(name), &mut files);
    }
    files
}

fn collect(base: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read a decoy dir") {
            collect(base, &entry.expect("a decoy entry").path(), files);
        }
    } else if path.is_file() {
        let relative = path
            .strip_prefix(base)
            .expect("inside the decoy")
            .to_path_buf();
        files.insert(relative, std::fs::read(path).expect("read a decoy file"));
    }
}

fn changed(before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>) -> Vec<String> {
    let mut names: Vec<&PathBuf> = before.keys().chain(after.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| before.get(*name) != after.get(*name))
        .map(|name| name.display().to_string())
        .collect()
}
