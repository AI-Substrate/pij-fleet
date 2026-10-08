# Difficulty ledger

Every difficulty encountered while working on pij gets a row here, with a
severity. Every workaround becomes either an immediate fix (encode it) or a
wishlist entry tagged `stretch:`. See "Self-improvement loop" in `AGENTS.md`.

| ID | Date | Severity | Difficulty | Resolution / encoding |
|----|------|----------|------------|-----------------------|
| D-167-1 | 2026-10-08 | high | A peer told to work in a stream worktree issued relative-path edits; the agent tool resolved them against its own cwd (the main checkout), silently editing `main`. Caught by an empty `git status` in the worktree. | Reverse-applied the exact patch in main and re-applied it in the worktree. `stretch:` pij peer spawn for a worktree task should start the seat with cwd = the worktree, so relative paths cannot reach main. Until then: absolute worktree paths for every edit. |
| D-167-2 | 2026-10-08 | low | `harness checks` failed `test` because the shell sets both `NO_COLOR` and `FORCE_COLOR`; node prints a warning on stderr that `skills/flow-pair/test/cli-learning.test.ts` asserts is empty. | Run the gate under `env -u FORCE_COLOR`. `stretch:` have that test (or the gate) clear the conflicting colour env for its subprocess. |
