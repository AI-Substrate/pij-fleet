# Difficulty ledger

Every difficulty encountered while working on pij gets a row here, with a
severity. Every workaround becomes either an immediate fix (encode it) or a
wishlist entry tagged `stretch:`. See "Self-improvement loop" in `AGENTS.md`.

| ID | Date | Severity | Difficulty | Resolution / encoding |
|----|------|----------|------------|-----------------------|
| D-167-1 | 2026-10-08 | high | A peer told to work in a stream worktree issued relative-path edits; the agent tool resolved them against its own cwd (the main checkout), silently editing `main`. Caught by an empty `git status` in the worktree. | Reverse-applied the exact patch in main and re-applied it in the worktree. `stretch:` pij peer spawn for a worktree task should start the seat with cwd = the worktree, so relative paths cannot reach main. Until then: absolute worktree paths for every edit. |
| D-167-2 | 2026-10-08 | low | `harness checks` failed `test` because the shell sets both `NO_COLOR` and `FORCE_COLOR`; node prints a warning on stderr that `skills/flow-pair/test/cli-learning.test.ts` asserts is empty. | Run the gate under `env -u FORCE_COLOR`. `stretch:` have that test (or the gate) clear the conflicting colour env for its subprocess. |
| D-167-3 | 2026-10-08 | medium | To honour the C12 load limit an agent stopped its own `harness checks` with `pkill -f 'harness checks'`; command-line patterns match every seat's run on the machine, so a peer's gate may have been killed. | Stop only what you started: hold the PID (`$!`/background job id) and kill that process group. `stretch:` a C12 wrapper that checks load before starting and owns its process group, so stopping is never a pattern match. |
| D-pa-1 | 2026-10-09 | low | A daemon test fixture named its temp git repo `pid-<clock nanos>`. The macOS clock only changes every microsecond, so two tests starting together got the same directory and one `git init` failed with "File exists": the suite failed in 2 of 3 runs. | A per-process atomic counter names the directory (`pa_watchdog_tests.rs`). Rule: never derive a unique name from the clock alone. |
