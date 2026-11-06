# Plan 209 evidence: fleet follow-ups

Phase `ph-00d1`, written by pij-local-warbler (coder), 2026-11-06.

**Proof is CI.** pm-001's 22:36 rule: two green CI runs on the exact head are the proof, and local 50-run loops are skipped at the fleet's load. Before that rule, the loops below had already been run; they are kept as extra evidence. Local builds ran only through `just shared-build` into fleet-194, compiled under `nice -n 19 taskpolicy -b`, and test binaries ran under `nice -n 19` alone (F63). TS tests are CI-only (C25).

## Gates

| Gate | Result |
|---|---|
| CI on the final head, twice (`check (22)`, `check (24)`, `rust-lint`, `rust-test`, `delivers`) | see the PR body for the run ids |
| `pij-skill-check` (local and the new CI step) | all green |
| `ddocs-check`, `local-path-check` | clean |

## AC1 (F27): the flakes, at their roots

| Test | Cause | Fix |
|---|---|---|
| `claude_heal_scripts` `copilot_status_line_hands_the_payload_to_pij_rs_in_one_process` | with pij-rs missing, the script exits without reading stdin, so the payload write races EPIPE | EPIPE on that write is not a failure; the record asserts the payload |
| `claude_inbound` `daemon_boot_repairs_once_and_publishes_one_change_event` | `ctrl_c()` arms SIGINT on its first poll, after "listening on" is printed; an early SIGINT killed the daemon (a production race) | the daemon installs its SIGINT handler before printing readiness |
| `pane_observer` `real_tmux_pane_observation_blocks_on_a_visible_composer` | `kill-server` exits non-zero when the server is already gone | teardown asserts no server is left |
| `control_mode` `a_stalled_control_client_is_killed_and_replaced` (also on main, run 42154126103) | automatic-rename moved the window name from printf to sleep between two reads | the test window is named |
| watchdog smoke teardown | the fixture SIGKILLed the daemon 3 s after SIGINT, inside the daemon's own 5 s drain | the daemon's stop waits 10 s (scaled) |
| reaper tests | none found failing | dropped: the 13 `reaper::` tests passed 50 runs in a row niced (load 29 to 51) and three full daemon-lib runs under `taskpolicy -b` (load 25 to 49) |

Extra local evidence (before the 22:36 rule): 50 of 50 each for claude_heal, claude_inbound, control_mode and pane_observer at load 24 to 43, niced.

## AC2 (F28): one test-only time scale

`pij_testkit::time::scaled` and `harness/driver/time-scale.ts` read `PIJ_TEST_TIME_SCALE` (default 1; blank is 1; below 1 or not a number refuses). Registration's real-`ps` deadline scales in `cfg(test)` only; `PROCESS_OBSERVATION_TIMEOUT` stays 1 s in production. `registration::` (94) and the native `ps` test passed under `taskpolicy -b` with the scale at 8; at load 25 they also passed unscaled, since #134's single-pid `ps` made them fast, so no failing control could be shown that day.

## AC3 (F26): the skill check

20 stale markers since plan 175: reworded doctrine is matched in its new words, reversed doctrine is retired with the reason in a comment, and `routes/peer.md` is reflowed under its budget (text identical modulo whitespace). The check runs in `harness checks` (sensor `pij-skill`), `just self-check` and CI. Deleting a routed module makes it exit 1.

## AC4 to AC6, AC9 to AC16

| AC | Item | Test |
|---|---|---|
| AC4 | #131 N002: the card's recorder is the one at its time | store `a_later_recording_by_another_seat_names_that_seat_with_its_time` |
| AC5 | #129: the digest's role queue is the PA's projects, else its prime's, else all only under the designated prime; a failed scope read shows none (#144 F002) | store `role_requests_left_delivering_are_recovered_at_boot` (scoped reads), `pa_watchdog` `the_digest_reads_the_pas_projects_...`, `a_failed_scope_read_shows_no_queue_rather_than_every_project`, `a_failed_prime_lookup_shows_no_queue_rather_than_every_project` (service level) |
| AC6 | #132 F005 B: a late prompt admission is recorded after the action | `memory_turn_http` `a_late_admission_is_recorded_after_the_action_never_withdrawn` |
| AC9 | F26 in CI, F31, F48, F51; F30, F41-F44, F49, F50 via plan 213; F30 shim test | `shim_forward` `a_memory_add_with_its_body_on_stdin_reaches_pij_rs`, `a_memory_add_prints_one_line`, the HUP and USR1 (EXIT-trap drain) cases in `shared-build.test.ts` |
| AC10 | F31, F32 (in #138's merge), F34 `--slug` | `a_given_slug_names_the_file_and_must_be_plain`, `a_memory_note_is_filed_under_the_local_date` (#138) |
| AC11 | F35-F38 | `add_help_names_the_required_flags`, `add_help_says_each_add_is_its_own_commit`, `a_role_refusal_names_the_flag_for_each_gap_not_a_hand_edit`, `a_role_refusal_lists_every_gap_at_once`; real-ddocs receipt for F37 below |
| AC12 | F65: the Delivers line is checked before the merge | `tests/delivers_check.rs`; the `delivers` workflow on this PR |
| AC13 | F67: `lineage add --batch`; diagnostics are phase-local, not exhaustive (#144 N001) | `a_lineage_batch_is_one_commit_all_or_nothing` |
| AC14 | F41 | `delivered_prints_the_edits_for_the_docs_pr` |
| AC15 | F40 | writer test: the foreign-edit refusal's next step names the prime |
| AC16 | F63: shared-build compiles tests in QoS, runs them nice-only; `cargo test --doc` is one nice-only step (#144 F001) | `shared-build.test.ts` scheduling and doc-only cases (the fake cargo refuses `--doc --no-run`, as cargo 1.98 does) |

### AC11, F37: real ddocs

06 Nov 23:24, read-only, head 464e061. A copy of `crates/daemon/tests/fixtures/community-roles/coder.dd.json` with `meta.value.kind`, `scope` and `standing_packet` removed, in a scratch directory with the worktree's `.dd/schemas`, run with the daemon's own arguments: `ddocs --json validate role.dd.json --depth 0` (`~/.npm-global/bin/ddocs`, the path the daemon resolves). Control: the unedited fixture returns `ok` with no issues.

```text
exit 1  error.code E402  details.counts.error 3
issues: $.sections[meta].value.kind  missing required field "kind"
        $.sections[scope]            missing required section "scope"
        $.sections[standing_packet]  missing required section "standing_packet"
next_action: Fix <path> at $.sections[meta].value.kind, then re-run `ddocs validate role.dd.json`.
```

So ddocs reports every gap in one refusal, and the formatter already printed them all. The fix-one-and-retry loop came from the last line, ddocs' `next_action`, which names only the first gap. `ddocs_message` now replaces it, for `pij/role`, with one line naming the flag for every gap; `a_role_refusal_names_the_flag_for_each_gap_not_a_hand_edit` fails on main. `a_role_refusal_lists_every_gap_at_once` guards the issue lines and would also pass on main. `three_gaps()` is this envelope minus fields the formatter does not read.

### Repair round (#144 verdicts at 464e061)

- F001: `cargo test --doc` got `--no-run` appended, which cargo refuses. Doc-only tests now skip the compile step and run once, nice-only. A hand probe against a fake cargo that refuses the pair: the old script exits 101, the new one runs `test -p x --doc` once.
- F002: `held_projects` kept no error, so a failed read became "holds nothing" and the prime fallback widened the digest to every project. The scope read now fails as a whole and the digest logs `queued role requests unavailable` and shows none; an empty scope reads nothing (marten's note). The service test reads `["req-alpha"]` with the holdings readable, then nothing once that read fails, while the prime and request reads still answer; before the fix it returned both projects. A guard for the prime lookup (ibex S001): the PA and parent hold nothing under the designated prime, so the working lookup shows both projects, and with `prime_designation` broken it shows nothing (the old code also failed closed there). The parent's holdings read is not failed on its own: it is the PA's query on the same table, keyed only by seat, so no fixture breaks one without the other.
- F48 residual: the EXIT trap drains a running cargo group. A hand probe with SIGUSR1 (untrapped; bash still runs the EXIT trap): the old script let the worker tick on after exit (4 to 9 ticks in 1 s), the new one stops it (3 to 3).
- N001 and N002: AC13 and AC11 are reworded as above.

F44 (shim cost), five runs at load 26 (06 Nov 20:12): `pij whoami --json` 0.43 to 0.84 s against 0.04 to 0.39 s for `pij-rs`; `pij claims` 0.08 to 0.50 s against 0.05 to 0.07 s. Plan 213 (#147) since hands the community verbs to `pij-rs` before tsx loads.

Security note on #132: the one runtime machine alias in `docs/plans/201-role-memory/assets/evidence/private-run.txt` (line 15) is replaced with `<machine>`, by pattern, without the value being printed anywhere.

## Cost (AC7)

| Change | Steady-state cost |
|---|---|
| AC1 to AC3, AC11, AC14, AC15 | tests, messages and the gate only; no production launch |
| AC4 | the address read returns one more column; same reads |
| AC5 | per PA digest (idle edge or prime-active, never per tick or seat): at most two `open_assignments` reads and one prime read for the scope, one `role_requests_queued` seek per project instead of one global read; an empty or failed scope reads no requests |
| AC6 | one tokio task per memory turn (the prompt's admission, which ran anyway) and one awaited join after a bound-ended turn; no timer, poll or launch |
| AC13 | one writer commit for N past holdings (N commits before) |
| AC16 | one more `cargo` and `nice` launch per `cargo test` command in shared-build (none for `--doc`) |
| AC12, AC3 in CI | a `delivers` job per PR event and a skill-check step; nothing on the machine |

No new daemon launch site (`spawn_sites.rs` green in CI). Live daemon at rest, 06 Nov 23:03:54 to 23:04:54 (pid 27104, load 16): `data.spawns.total` 920 to 962, 42 launches in 60 s (0.70/s); `just spawn-rate --seconds 20`: 50.7/s machine-wide, `pij-rs daemon` 0.2/s. The after-deploy reading is owed at merge.
