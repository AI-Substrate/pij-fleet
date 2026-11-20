//! F241: the one-time sweep of tmux sockets and attach locks pij's test
//! fixtures leaked into tmux's shared directory (`pij_testkit::tmux_sweep`).
//!
//! `pij-tmux-fixture-sweep [--dir <path>]` is a dry run: it prints counts by
//! prefix, kind and verdict, and a sample of what it would remove. Add
//! `--apply` to remove the stale entries. Run it as `just tmux-fixture-sweep`.
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use pij_testkit::tmux_sweep::{self, Verdict};

const SAMPLE: usize = 5;

fn main() -> ExitCode {
    let mut apply = false;
    let mut dir = pij_testkit::tmux::socket_dir();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--apply" => apply = true,
            "--dir" => match args.next() {
                Some(path) => dir = PathBuf::from(path),
                None => return usage("--dir needs a path"),
            },
            other => return usage(&format!("unknown argument {other:?}")),
        }
    }
    let holders = tmux_sweep::Holders::read();
    match &holders {
        tmux_sweep::Holders::Known(held) => {
            println!("holders: {} bound Unix socket files", held.len());
        }
        tmux_sweep::Holders::Unknown(why) => {
            println!(
                "holders: unknown ({why}); no socket counts as unheld, nothing of them is removed"
            );
        }
    }
    let report = match tmux_sweep::sweep(&dir, SystemTime::now(), apply, &holders) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("pij-tmux-fixture-sweep: {}: {error}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} {} (prefixes: {})",
        if apply { "swept" } else { "dry run of" },
        dir.display(),
        tmux_sweep::FIXTURE_PREFIXES.join(" ")
    );
    for ((prefix, kind, verdict), count) in report.counts() {
        println!("{count:>8}  {prefix:<12} {kind:?} {verdict:?}");
    }
    println!(
        "{:>8}  ignored (default, its lock, every other name)",
        report.ignored
    );
    let stale: Vec<_> = report
        .entries
        .iter()
        .filter(|entry| entry.verdict == Verdict::Stale)
        .collect();
    println!(
        "stale: {} ({} sockets, {} locks)",
        stale.len(),
        stale
            .iter()
            .filter(|e| e.kind == tmux_sweep::Kind::Socket)
            .count(),
        stale
            .iter()
            .filter(|e| e.kind == tmux_sweep::Kind::Lock)
            .count()
    );
    for entry in stale.iter().take(SAMPLE) {
        println!("  e.g. {}", entry.name);
    }
    if apply {
        println!("removed: {}", report.removed);
        for (name, error) in &report.failed {
            eprintln!("  could not remove {name}: {error}");
        }
        if !report.failed.is_empty() {
            return ExitCode::FAILURE;
        }
    } else {
        println!("nothing removed; rerun with --apply to remove the stale entries");
    }
    ExitCode::SUCCESS
}

fn usage(problem: &str) -> ExitCode {
    eprintln!(
        "pij-tmux-fixture-sweep: {problem}\nusage: pij-tmux-fixture-sweep [--dir <path>] [--apply]"
    );
    ExitCode::from(2)
}
