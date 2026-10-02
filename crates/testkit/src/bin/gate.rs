//! `pij-gate` — the one command (ac-0007).
//!
//! ```text
//! ~/.cargo/bin/cargo run --locked -p pij-testkit --bin pij-gate
//! ```
//!
//! `--locked` on the OUTER invocation matters as much as the `lock` stage below:
//! without it, the `cargo run` that builds the gate can quietly repair a stale
//! lock before the gate ever starts, and the stage then inspects a tree the gate
//! itself just fixed.
//!
//! Five stages, in the order that fails cheapest first:
//!
//! 1. **toolchain** — the running rustc equals the pin, or nothing below means
//!    anything (workshop 001 R6a as amended).
//! 2. **lock** — `cargo metadata --locked` proves `Cargo.lock` matches the
//!    manifests. Review caught a commit whose lock predated a new dependency:
//!    every local build passed because cargo silently UPDATED the lock, while a
//!    fresh `--locked` checkout could not build at all. A gate that mutates the
//!    tree it is judging can report green on something nobody else can compile.
//! 3. **fmt** — `cargo fmt --check`.
//! 4. **clippy** — `-D warnings`, all targets.
//! 5. **test** — `cargo test --workspace`.
//! 6. **arch** — the crate graph against the committed allow-list.
//!
//! It runs EVERY stage even after one fails, and prints a per-stage verdict at
//! the end: an agent that has to re-run a gate five times to find five problems
//! has been given a worse instrument than one that answers in a single pass. The
//! exit code is still the answer — non-zero means the task is not done.
//!
//! `CARGO_INCREMENTAL=0` is set for the children: incremental artifacts are pure
//! cost on a machine where several agents build the same tree.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pij_testkit::{arch, lockfile, toolchain};

struct Stage {
    name: &'static str,
    args: Vec<&'static str>,
}

fn main() -> ExitCode {
    let cargo = toolchain::cargo_path();
    let root = arch::workspace_manifest_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    println!("pij-gate: {} · cargo {}", root.display(), cargo.display());

    let mut failures: Vec<&'static str> = Vec::new();
    let mut verdicts: Vec<(&'static str, bool, String)> = Vec::new();

    // Stage 1 — the toolchain, in-process: it decides whether the rest is
    // evidence at all.
    match toolchain::assert_pinned(&root, &cargo) {
        Ok(version) => verdicts.push(("toolchain", true, format!("rustc {version} == pin"))),
        Err(message) => {
            verdicts.push(("toolchain", false, message));
            failures.push("toolchain");
        }
    }

    // Stage 2 — the lockfile, in-process so the check cannot be weakened by an
    // argument list drifting (its first version used `--no-deps`, which returns 0
    // against a stale lock; review caught it).
    //
    // Two questions, because wave 3 shipped a head that passed the first and
    // failed the second: does the lock match the manifests, AND is the lock on
    // disk the one that is COMMITTED? A gate run against a locally-repaired lock
    // judges a tree nobody else can clone — which is exactly what happened, six
    // green runs in a row, because the outer `cargo run` repaired the file before
    // the gate started. Ruled a governance item by the prime: **a gate that
    // repairs its own input is a formatter that reports PASS.**
    match lockfile::check(&root, &cargo) {
        Ok(()) => match lockfile::committed(&root) {
            Ok(()) => {
                verdicts.push((
                    "lock",
                    true,
                    "Cargo.lock matches the manifests, and the tree".to_string(),
                ));
            }
            Err(message) => {
                verdicts.push(("lock", false, message));
                failures.push("lock");
            }
        },
        Err(message) => {
            verdicts.push(("lock", false, message));
            failures.push("lock");
        }
    }

    // `--locked` on EVERY cargo stage, not just the outer invocation. Any of
    // these will silently rewrite a stale lock and then pass, which makes the
    // stage a repair step wearing a check's name.
    let stages = [
        Stage {
            name: "fmt",
            args: vec!["fmt", "--all", "--check"],
        },
        Stage {
            name: "clippy",
            args: vec![
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        },
        Stage {
            name: "test",
            args: vec!["test", "--locked", "--workspace"],
        },
    ];

    for stage in &stages {
        let status = Command::new(&cargo)
            .args(&stage.args)
            .current_dir(&root)
            .env("CARGO_INCREMENTAL", "0")
            // The test stage's ONE self-healing vector: `UPDATE_GOLDENS` makes a
            // failing golden re-record itself and pass. Removed from the child's
            // environment rather than trusted to be unset, because an exported
            // variable in the caller's shell would otherwise turn the whole stage
            // into a recording session that reports PASS.
            .env_remove("UPDATE_GOLDENS")
            .status();

        match status {
            Ok(status) if status.success() => {
                verdicts.push((stage.name, true, format!("cargo {}", stage.args[0])));
            }
            Ok(status) => {
                verdicts.push((stage.name, false, format!("exit {status}")));
                failures.push(stage.name);
            }
            Err(error) => {
                verdicts.push((stage.name, false, error.to_string()));
                failures.push(stage.name);
            }
        }
    }

    // Stage 5 — the architecture, in-process: it is pure over `cargo metadata`,
    // so there is nothing to shell out to.
    match (arch::allowlist(), arch::workspace_graph()) {
        (Ok(allowlist), Ok(graph)) => {
            let violations = arch::check(&graph, &allowlist);
            if violations.is_empty() {
                verdicts.push((
                    "arch",
                    true,
                    format!("{} crates, 0 violations", graph.crates.len()),
                ));
            } else {
                for violation in &violations {
                    eprintln!("  - {violation}");
                }
                verdicts.push(("arch", false, format!("{} violation(s)", violations.len())));
                failures.push("arch");
            }
        }
        (Err(error), _) | (_, Err(error)) => {
            verdicts.push(("arch", false, error.to_string()));
            failures.push("arch");
        }
    }

    println!("\npij-gate verdict");
    for (name, ok, detail) in &verdicts {
        println!(
            "  {:<9} {}  {detail}",
            name,
            if *ok { "PASS" } else { "FAIL" }
        );
    }

    if failures.is_empty() {
        println!("\npij-gate: all stages green");
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "\npij-gate: {} stage(s) failed — {}. The task is not done.",
        failures.len(),
        failures.join(", ")
    );
    ExitCode::FAILURE
}
