//! `pij-arch-check` — the architecture drift gate (workshop 001 R2).
//!
//! Reads the live workspace graph and judges it against
//! `crates/testkit/arch-allowlist.toml`. Exit 0 means the crate graph is still
//! what the workshop says it is; exit 1 names every refused edge and what to do
//! about it. Stage 5 of `pij-gate`.

use std::process::ExitCode;

use pij_testkit::arch;

fn main() -> ExitCode {
    let allowlist = match arch::allowlist() {
        Ok(allowlist) => allowlist,
        Err(error) => {
            eprintln!("arch-check: {error}");
            return ExitCode::FAILURE;
        }
    };

    let graph = match arch::workspace_graph() {
        Ok(graph) => graph,
        Err(error) => {
            eprintln!("arch-check: {error}");
            return ExitCode::FAILURE;
        }
    };

    let violations = arch::check(&graph, &allowlist);
    if violations.is_empty() {
        println!(
            "arch-check: ok — {} crates, {} direct edges, 0 violations",
            graph.crates.len(),
            graph.crates.iter().map(|c| c.deps.len()).sum::<usize>()
        );
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "arch-check: {} architecture violation(s) — the crate graph has drifted from workshop 001",
        violations.len()
    );
    for violation in &violations {
        eprintln!("  - {violation}");
    }
    eprintln!(
        "\nThe allow-list is crates/testkit/arch-allowlist.toml; every row carries the reason it exists."
    );
    ExitCode::FAILURE
}
