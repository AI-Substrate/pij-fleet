# Plan 184 receipts

All commands ran in the plan worktree (`fix/staged-submit-deadlock`). "Main" is
the plan commit `3bfe24e` (pre-fix code) in a scratch worktree, with only the
new test files copied in.

## Red on main → green on the branch

| test | main (quoted) | branch |
|---|---|---|
| `pointer::worker::tests::plan184::own_staged_leftover_fails_its_command_by_name_and_tells_the_sender` | `panicked … pij's own leftover fails the command` (job held, not failed) | ok |
| `…::composer_busy_hold_backs_off_and_never_reserves_the_pane` | `composer-busy backs off on the job's own attempt count  left: [(JobId(1), 0ns)]  right: [(JobId(1), 8s)]` | ok |
| `…::composer_busy_backoff_is_capped_at_a_minute` | `left: [(JobId(1), 0ns)]  right: [(JobId(1), 60s)]` | ok |
| `…::held_command_fails_by_name_at_the_hold_limit_and_tells_its_sender` | `panicked … terminal at the hold limit` | ok |
| `…::failed_control_head_unblocks_the_messages_queued_behind_it_in_order` (real sqlite queue) | `every queued message, in order, once the control head failed  left: []  right: [Msg { … "message 1" … }, …]` | ok |
| `…::held_body_tells_its_sender_once_at_the_hold_limit_and_stays_queued` | `told once, not every attempt  left: 0  right: 1` | ok |
| `crates/daemon/tests/held_delivery_launches.rs::a_held_delivery_waits_without_launching_anything` | `20 held attempts launched [("tmux.atomically reserve staged submit", 20), ("tmux.restore pane input and release ownership", 20)]` | ok |
| `crates/tmux/tests/adapter.rs::staging_at_the_measured_launch_tail_still_submits` | `a stage at the launch tail must commit: … paste owned submit buffer failed (exit 75) … staged-submit recovery also failed: tmux: staged submit ownership for pane "%0" no longer matches token …` | ok |

These were green on both sides by design:
- `…::own_leftover_plus_one_keystroke_stays_a_human_draft` is the composer-gate
  guard (R1). Main never touched a draft either.
- `slow_launches_strand_a_short_command_with_the_recovery_notice` characterizes
  the diagnosis.

These tests use APIs new in this plan, so they cannot compile on main; they
cover the same behaviour as the rows above:
- gate tests in `crates/harnesses/src/interaction.rs`: own leftover cleared,
  grace respected, one keystroke or other text is human, the verb refuses;
- matcher tests in `crates/core/src/ports.rs`, using both live renderings as
  fixtures;
- real-tmux `clear_staged_leftover_*` tests;
- park-notice hold-limit tests.

## Mutants (all killed, all restored)

The runner applies each mutation, runs its tests, and restores the file,
asserting byte equality. Afterwards `diff -rq` of `crates/` against a
pre-mutant copy was identical.

| mutant | killed by |
|---|---|
| M1 own text treated as human (both recognitions → false) | `own_staged_leftover_fails_its_command…` FAILED; `own_exact_leftover_is_cleared…` FAILED |
| M2 no backoff (composer-busy → zero delay) | `composer_busy_backoff_is_capped…` FAILED; `composer_busy_hold_backs_off…` FAILED |
| M3a no sender notice on a failed command (parked events not published) | `held_command_fails_by_name…` FAILED; `own_staged_leftover_fails…` FAILED |
| M3b no hold-limit fact for a held body | `held_body_tells_its_sender_once…` FAILED |
| M3c follower ignores hold-limit facts (subscription and sweep) | `the_follower_notifies_a_hold_limit_as_it_is_published` FAILED |
| M4 control failure still blocks the queue (limit arm disabled) | `failed_control_head_unblocks…` FAILED |
| M5 reserve before reading (look skipped) | `a_held_delivery_waits_without_launching_anything` FAILED; `composer_busy_hold_backs_off_and_never_reserves_the_pane` FAILED |
| M6 stage bound 1 s, watchdog 2 s | `staging_at_the_measured_launch_tail_still_submits` FAILED |

## Cost (ac-0004)

- **Before, live machine daemon** (read-only `pij-rs --json ping`, 30 s apart):
  `tmux.atomically reserve staged submit` 16419 → 16431 and
  `tmux.restore pane input and release ownership` 16419 → 16431. That is
  0.4/s each, 0.8/s together, out of 1.5/s total.
- **After, measured in-process on the real adapter**: 20 held attempts on a
  private `-L` server launched nothing
  (`held_delivery_launches.rs`; on main, 40).
- **Steady state after the fix:** zero launches per held attempt; attempts
  back off to one a minute per held seat. A submittable attempt costs what it
  did before (reserve, three pastes, commit). A recognised leftover costs
  reserve, one clear, and restore, once.
- **The deployed live reading** is the prime's, after merge and bounce.

## Review round (#91): B1, S1–S5, and the Copilot control kind

After rebasing onto main 8687560 (plan 186 merged), "old" is the pre-review
head `8317d09` (the rebased `cff8819`) in a scratch worktree, with the new
`plan184.rs` copied in. The scratch copy also made `InteractionGate::with_clock`
public so the walrus test could compile. It left out the residue test, whose
fake hook only the fix adds.

### Red on old → green on the branch

| test | old (quoted) |
|---|---|
| `human_typing_never_counts_toward_the_limit_and_waits_out_the_grace` (B1) | `typing never fails a command: Some("undelivered:hold-limit: /compact was still held (human-typing) after 12 attempts")` |
| `the_walrus_deploy_clears_its_own_leftover_and_drains_the_queue_in_order` (S5) | `held, not failed  left: ("failed", Some("undelivered:hold-limit: /compact was still held (human-typing) after 12 attempts"))  right: ("pending", None)` |
| `a_held_body_tells_its_sender_once_then_fails_by_name` (S1, real queue) | `told once, across passes on the real queue  left: 5  right: 1` |
| `a_copilot_control_held_unrecognized_fails_by_name_at_the_limit` (plan 186 kind) | `terminal at the hold limit` (never terminal) |
| `held_command_fails_by_name_after_seven_minutes_held_and_tells_its_sender` | `failed early, at 363000 ms` |
| `composer_busy_backs_off_on_its_own_holds_to_a_minute_and_never_reserves` (attempt 9180) | `left: []  right: [1, 2, 4, 8, 16, 32, 60, 60]` (failed at once on legacy attempts) |
| `failed_control_head_unblocks_the_messages_queued_behind_it_in_order` | `left: []` |

Green on old by design:
- `own_staged_leftover_fails…` and `a_copilot_control_clears_its_own_leftover…`
  (full-text recognition was already there);
- `own_leftover_plus_one_keystroke…`, the R1 guard.

S2, S3 and S4 use APIs the fix introduces. Their red is the mutant table below.
An old-code variant of the real-tmux S4 test was inconclusive: the stage did
not trip the old bound under that wrapper.

### Mutants (all killed; `diff -rq crates/` identical afterwards)

| mutant | killed by |
|---|---|
| B1a typing counts toward the limit | `human_typing_never_counts…` |
| B1b typing retries on the next tick (ignores `retry_after`) | `human_typing_never_counts…` |
| S1 hold-limit fact without `msg_id` | `a_held_body_tells_its_sender_once…` (real queue) |
| S2 mutant A: clear decided on the look with `staged_leftover(text)` | `the_clear_is_decided_on_the_owned_read_not_the_look` |
| S3 no re-read after the clear | `a_clear_that_does_not_read_blank_sends_nothing_more`; `a_clear_that_leaves_residue_sends_nothing_more_and_fails_by_name` |
| S4 notice pasted inside the open paste | `an_abort_between_chunks_closes_the_paste_and_records_what_was_pasted` (real tmux) |
| S4 no record of what was pasted | `a_partial_stage_is_recognised_from_what_was_actually_pasted` |

### Scoped gates on the branch
- `cargo test -p pij-core -p pij-harnesses -p pij-tmux -p pij-testkit -p pij-daemon --no-fail-fast`:
  exit 0, 74 test binaries ok.
- `cargo fmt --all --check`: ok.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: ok.
