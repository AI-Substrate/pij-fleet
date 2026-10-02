//! The lockfile check must fail on the incident it was written for.
//!
//! The first version of this stage used `cargo metadata --locked --no-deps`,
//! which returns **0** against a lock that is missing a dependency — so the check
//! passed the exact commit it existed to catch, and a reviewer had to prove it by
//! building the gate from one export and running it against a doctored one.
//!
//! These tests build a two-crate workspace in a temp directory, so they exercise
//! the real `cargo --locked` behaviour without depending on the network, the
//! registry cache, or the state of this repo's own lock.

use std::path::{Path, PathBuf};

use pij_testkit::{lockfile, toolchain};

/// A minimal path-only workspace: `a` depends on `b`, so the lock has an edge
/// that can be removed.
fn scaffold() -> PathBuf {
    let root = pij_testkit::fresh_dir("pij-lockcheck");
    std::fs::create_dir_all(root.join("a/src")).expect("mkdir a");
    std::fs::create_dir_all(root.join("b/src")).expect("mkdir b");

    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"a\", \"b\"]\n",
    )
    .expect("write workspace manifest");
    std::fs::write(
        root.join("a/Cargo.toml"),
        "[package]\nname = \"lockcheck-a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nlockcheck-b = { path = \"../b\" }\n",
    )
    .expect("write a");
    std::fs::write(
        root.join("b/Cargo.toml"),
        "[package]\nname = \"lockcheck-b\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write b");
    std::fs::write(root.join("a/src/lib.rs"), "pub fn a() {}\n").expect("write a src");
    std::fs::write(root.join("b/src/lib.rs"), "pub fn b() {}\n").expect("write b src");
    root
}

fn generate_lock(root: &Path, cargo: &Path) {
    let status = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run cargo metadata");
    assert!(status.success(), "the fixture workspace must resolve");
}

#[test]
fn a_current_lock_passes() {
    let cargo = toolchain::cargo_path();
    let root = scaffold();
    generate_lock(&root, &cargo);

    lockfile::check(&root, &cargo).expect("a freshly generated lock is current");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_stale_lock_fails_and_is_left_byte_identical() {
    let cargo = toolchain::cargo_path();
    let root = scaffold();
    generate_lock(&root, &cargo);

    // Remove exactly one package entry — the shape of a manifest that gained a
    // dependency without its lock being regenerated and committed.
    let lock_path = root.join("Cargo.lock");
    let current = std::fs::read_to_string(&lock_path).expect("read lock");
    let stale = current.replace(
        "\n[[package]]\nname = \"lockcheck-b\"\nversion = \"0.1.0\"\n",
        "\n",
    );
    assert_ne!(stale, current, "the fixture lock must contain the edge");
    std::fs::write(&lock_path, &stale).expect("write stale lock");

    let error = lockfile::check(&root, &cargo).expect_err("a stale lock must fail");
    assert!(
        error.contains("Cargo.lock does not match the manifests"),
        "the failure must name the problem: {error}"
    );
    assert!(
        error.contains("COMMIT"),
        "...and the fix, because repairing it locally is what hid the bug: {error}"
    );

    // The check must not REPAIR what it is judging. A gate that mutates the tree
    // reports green on something nobody else can compile.
    assert_eq!(
        std::fs::read_to_string(&lock_path).expect("read lock"),
        stale,
        "the check must leave Cargo.lock byte-identical"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_no_deps_flag_would_have_made_this_check_useless() {
    // Kept as an executable note, not folklore: `--no-deps` is the obvious flag
    // for "I only want the workspace", and with it the check returns 0 against
    // the very lock the test above proves is stale. This asserts the trap still
    // behaves the way the fix assumes, so nobody re-adds the flag as a speedup.
    let cargo = toolchain::cargo_path();
    let root = scaffold();
    generate_lock(&root, &cargo);

    let lock_path = root.join("Cargo.lock");
    let current = std::fs::read_to_string(&lock_path).expect("read lock");
    std::fs::write(
        &lock_path,
        current.replace(
            "\n[[package]]\nname = \"lockcheck-b\"\nversion = \"0.1.0\"\n",
            "\n",
        ),
    )
    .expect("write stale lock");

    let with_no_deps = std::process::Command::new(&cargo)
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(&root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run cargo metadata --no-deps");

    assert!(
        with_no_deps.success(),
        "if --no-deps ever starts failing on a stale lock, this note is obsolete \
         and the comment in lockfile.rs should be revisited"
    );
    assert!(
        lockfile::check(&root, &cargo).is_err(),
        "...while the real check must still fail on the same tree"
    );

    let _ = std::fs::remove_dir_all(root);
}
