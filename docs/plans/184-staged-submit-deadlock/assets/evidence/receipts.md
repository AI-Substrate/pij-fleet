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
