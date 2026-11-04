# Plan 195 phase 2 evidence: holding roles on a private daemon

Phase `ph-95b2`, written by pij-brisk-marten (coder), 2026-11-04. Script [`private-run.sh`](private-run.sh), transcript [`private-run.txt`](private-run.txt). Home and worktree paths are shortened to `~` and `<worktree>`.

The script runs a private daemon: temp `HOME` and state, a private `-L` tmux server with two panes, port 17902, scratch repos, and two scratch seats bound to those panes. It never touches the machine daemon, `~/.pij-rs` or the live governance branch. The binary is a debug `pij-rs` from this branch after merging phase 1 (bf78bff), with the real ddocs and the embedded `pij/*` schemas (C9).

## Cost (bp-0007, AC11)

All readings come from one daemon process (pid 6787).

| Window | Launches | By operation |
|---|---|---|
| At rest, 60 s | 0 | none |
| One `directory assign reviewer/114 <seat>` | 10 | git.community 7, ddocs.community 2, tmux.rename-window 1 |
| One `directory release` | 9 | git.community 7, ddocs.community 2 |
| One `pij directory` (the view) | 0 | none |
| One `pij close` of a seat holding a role | 11 | git.community 7, ddocs.community 2 for the holders commit; tmux.clear-options and tmux.pipe-pane detach are close's existing pane-tap teardown |

- The assign renamed the holder's window to `fleet:reviewer-114`, with one recorded tmux launch and no control-client write (ruling (b)).
- The close committed the `pm-001` holders row with `release_why: close` (`data.directory.committed`).
- Nothing runs on a timer. Every launch is caused by a verb, and the count scales with verb calls, never with seats or ticks.

## Gates

- The TS proof (typecheck, vitest including `generation-skill-coverage.test.ts`, the full `harness checks`) is CI on the PR head; this worktree has no `node_modules`.
- Shared-target hazard: other seats' `cargo test` runs in this target (from `~/pi-hacking/fleet-delivery`) overwrote this tree's `pij_core` and `pij_store` artifacts three times, which produced false compile errors and 13 false `pa_watchdog` failures. Cargo names workspace crates' artifacts relative to the workspace root, so worktrees with the same layout share artifact names. Local runs here were taken when no other cargo was running, after touching each crate's `lib.rs`.

## Review round 1 (pij-substantial-mammal, block at 94592ad)

- F001: an assign or release of a role applied none of that role's pending close-ends, yet cleared every pending mark. Now the write applies the role's pending ends to its own rows under the writer lock, and only the ends a commit carried lose their mark (`holders::apply_ends` returns them).
- F002: `from` had second precision and is part of a holding's key, so an assign, release and reassign within one second collided. Times are now UTC to the millisecond, and `holders::fresh_from` picks a `from` no row of that seat and instance has, under the writer lock. `check_rows` refuses two rows for one holding, and rebuild runs it before replacing anything; the store's `replace_assignments` also refuses a duplicate instead of keeping the first.
- The reviewer's three HTTP regressions are in `crates/daemon/tests/pr122_review/mod.rs`. Under the shared-target lock (`CARGO_INCREMENTAL=0`, `-p pij-store -p pij-daemon`): clippy `-D warnings` clean; 1122 passed, 0 failed in 50 binaries, including all 14 `directory_http` tests.
- Cost: unchanged. No new launch sites; each write is still one commit.
