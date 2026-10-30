# Recheck: PR #99 at `9f7b36e` (rebased on main `2e8082d`)

**Verdict: merge. 0 blockers.** Reviewer `pij-review-pr99`. Read-only; every probe and mutant was restored and the worktree is clean.

1. **B1 fixed.**
   - I re-added my original probe: tombstone the owner, run 30 ticks, lift the tombstone, run 3 ticks. Result: `bg.killed` count stays 1 throughout the wait, and the owner gets **exactly 1** final turn. Your `a_revived_owner_still_gets_its_sources_final_turn_once` passes too.
   - The refusal text has no timestamp, so it is identical every tick. `RepeatedFailureLog` (`lib.rs` boot wiring) therefore logs it once. That is checked by reading the code plus its unit test; I did not watch a live log.
2. **S1 fixed.**
   - `a_failing_pane_does_not_make_the_sweep_run_every_tick`, which is my probe, gives (3 errors, 1 sweep).
   - Mutant: putting the sweep back on `world_reconciled` makes it **FAIL**.
   - Mutant: changing `if owned.len() != listed` to `if true` makes `the_claiming_sweep_…` **FAIL** on the ledger-mtime assert. So "no rewrite on a no-op" has a test that catches it.
3. **N5: you are right, and my nit was wrong.** `crates/testkit/src/exec.rs:23` polls once with `Waker::noop()` and panics on `Pending`. Both `tmux_contract` callers (`adapter.rs:97`, `:161`) use it. `tokio::time::sleep` would return Pending, and it has no reactor either.
4. **N1 and N3.**
   - N1: `the_sweep_spares_a_sink_this_adapter_still_tracks` runs on a real private `-L` server. Dropping the tracked-tap veto (`.take(0)`) makes it **FAIL**, so the brake now has a witness.
   - N3: `newest` and `spine_head` share one `spine_head(pool)` that uses `COALESCE` plus `sql_u64`. No `expect` is left on that path.
5. **Fix commits and rebase.**
   - `git range-diff`: only the perf commit changed, and only by import and context lines around plan 184's `is_rendered_leftover` and `SUBMIT_STAGE_LIMIT`. Nothing semantic.
   - Tests on `9f7b36e`, all green: `cargo test -p pij-daemon` 880 passed, 0 failed. `-p pij-tmux -p pij-store -p pij-sidecars` passed, including the real-tmux contract and the tap tests.
   - `cargo fmt --check` is clean. `cargo clippy -p pij-tmux -p pij-store -p pij-sidecars -p pij-daemon -p pij-testkit --all-targets -D warnings` is clean.

**Not run:** the whole-workspace `just rust-check` and `harness checks`, the TypeScript stages (no `node_modules`), and the replay. My target dir is 8.0 G; 56 Gi was free before the builds.
