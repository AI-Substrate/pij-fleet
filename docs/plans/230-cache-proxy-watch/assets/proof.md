# Plan 230 proof receipts

Built at `20e7d932` (the code commits on main `81b459c4`); the later rebase onto main `48437352` (#242, docs only) left the code unchanged (now `63f2a9b1`). Binary built with `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- build -p pij-cli --bin pij-rs` and copied straight out to `/tmp/fire230/bin/pij-rs` (D-229-2).

Rig, all private; the prod daemon, port 7471 and the LaunchAgent were never touched:

- fake GitHub/Copilot upstream on 17681 (`/tmp/fire230/upstream.py`, removed after): `copilot_internal/user` stalls N s while a stall file holds N; `/v1/messages` answers 401 once while a flag file exists;
- fake gh (`PIJ_GH=/tmp/fire230/gh`) printing a fresh fixture token `gho_fake_<n>`; no real token was read or printed;
- sidecar: `pij-rs cache-proxy --port 17671 --state-dir /tmp/fire230/proxy-state` with `PIJ_CACHE_PROXY_GITHUB_API=http://127.0.0.1:17681`, account `fire230-tester`;
- daemon: `pij-rs daemon --bind 127.0.0.1:17661 --state-dir /tmp/fire230/state` (own store `/tmp/fire230/state/pij.sqlite`), `PIJ_TMUX_SERVER=fire230`, `PIJ_CACHE_PROXY_URL=http://127.0.0.1:17671`;
- prime: `pij-successful-haddock` (`orchestration prime set`); notices read from the private store's `fyis` table.

## Live rows

| Step | Setup | Observed |
|---|---|---|
| A | one request through the sidecar (upstream 401 once, then refresh and retry: a recheck is owed), `copilot_internal/user` stalled 45 s | status read 20 ms, `ready`, old `checked_at_ms`; Copilot spawn `pij-accepted-ostrich` proxied in 0.4 s. The 45 s stall is past the readiness GET's own 15 s timeout, so the background check ended `upstream_unreachable` at 05:21:54Z (`copilot_internal/user unreachable: error sending request`): one genuine notice, and one `ready (was upstream_unreachable)` when the stall was cleared. That is the readiness contract, not a watch flap. |
| B | the same, `copilot_internal/user` stalled 10 s (slow, inside the timeout) | 3 status reads during the stalled check: 16, 16, 15 ms, all `ready`; spawn `pij-nutritious-swordtail` proxied in 0.4 s; 2 watch ticks: `notices` 2 → 2, `slow.count` 0, state `ready`; upstream log: exactly one `user read` (05:23:44 stall 10 s, answered 05:23:54); `checked_at_ms` advanced after it landed |
| C1 | sidecar `SIGSTOP` for one watch tick (accepts, never answers), then `SIGCONT` | tick: state `ready`, `reachable` false, `error` `no answer within 1 s`, `slow` `{count 1, in_a_row 1}`, `notices` 2 → 2; next tick `ready`, `in_a_row` 0 |
| C2 | `SIGSTOP`, a Copilot spawn | `pij-unaware-salamander` plain in 2.4 s: `proxy did not answer: no answer within 1 s, twice`, fix names `pij-rs cache-proxy status`, `just cache-proxy-autostart` and `pij-rs revive <seat>` |
| C3 | `SIGSTOP` held for 2 watch ticks, then `SIGCONT` | 2nd failed read: state `not_listening`, `slow` `{count 3, in_a_row 2}`, `notices` 3; one FYI: `pij cache proxy is not answering at http://127.0.0.1:17671 (was ready): no answer within 1 s, on 2 reads in a row 30 s apart. Proxied seats' model calls may fail. Proxied seats: 4 (`just doctor` lists them). Fix: …`. After `SIGCONT`, one FYI `ready (was not_listening) … Proxied seats: 4 (`just doctor` lists them)` |
| D | sidecar stopped (port closed), later restarted | the next tick, at once: `not_listening`, `error` `connection refused`, one FYI `pij cache proxy is not listening at http://127.0.0.1:17671 (was ready): connection refused. Proxied seats' model calls fail. Proxied seats: 4 (`just doctor` lists them). Fix: start the sidecar …`; restarted: `ready`, notices 6 in all |

No notice in the store names a seat id: all 6 carry `Proxied seats: <n> (`just doctor` lists them)`.

Doctor (the recipe's `cache seats:` line, private binary first on `PATH`, private daemon address): `cache seats: proxied: pij-accepted-ostrich, pij-alert-peafowl, pij-nutritious-swordtail, pij-successful-haddock; plain: pij-unaware-salamander; slow reads: 3, last 2026-11-18T05:26:33.364Z`.

## Cost (k-3, k-4, k-5)

`pij-rs --json ping | jq .data.spawns` on the private daemon:

- across the whole run (3 Copilot spawns, every watch tick, the stopped reads and a gate retry): `total 63 → 72`. The +9 is `tmux.new-window` +3, `tmux.pipe-pane attach` +3, `tmux.set-options tap owner` +3: the existing per-spawn launches, 3 per spawn. No op from the watch, the gate or its retry.
- steady state, sidecar up, 5 Copilot seats: `total 72 → 72` over 60 s (two watch ticks), by-op diff empty.
- sidecar: one `copilot_internal/user` read per owed check; in step B, 3 status reads, a spawn gate and 2 watch ticks during a stalled check caused one upstream check. `gh launches` 2 (start, one refresh).

No new timer, launch or per-seat work: the background check replaces the inline one; the gate's retry is one more loopback GET, only after a timeout. `crates/testkit/tests/spawn_sites.rs` unchanged.

## Mutation receipts

Each guard mutated, its test run through `just shared-build -- test -p <crate> …`, the source restored after; every mutant failed its test.

| Mutant | Change | Test that failed |
|---|---|---|
| S1 inline check (main's behaviour) | `status()` awaits `run_check(false)` instead of spawning | `a_status_read_never_waits_on_a_stalled_readiness_check` (5.27 s), `an_aged_out_answer_starts_one_background_check_for_concurrent_reads` (`started.elapsed() < 100 ms`) |
| S2 no single-flight | spawn the background check without `try_lock_owned` | `an_aged_out_answer_starts_one_background_check_for_concurrent_reads` (`three reads, one check`: left 3). The integration test survives this one: on the owed-recheck path the recheck is consumed as the check starts, so the TTL path is the lock's own test |
| S3 stale rule off | `if_sent_with_current` applies every answer | `a_stale_402_from_the_old_account_never_latches_the_new_one` (state `QuotaExhausted`), `a_stale_2xx_from_the_old_account_never_clears_the_new_ones_402` (state `Ready`) |
| D1 one slow read is an outage (main's behaviour) | `DOWN_AFTER = 1` | `the_watch_declares_down_on_a_refusal_or_two_failed_reads_in_a_row` (`one timeout`: 1 notice) |
| D2 refusal debounced | `is_slow` also matches `Refused` | the same test (`a refusal`: 0 notices) |
| D3 no reset on a good read | `in_a_row = 0` removed | the same test (`timeout, ready, timeout`: 1 notice) |
| D4 no gate retry (main's behaviour) | the timed-out read is not read again | `the_gate_retries_a_timed_out_read_once` (seat `Plain`) |

The refusal row ("a refusal gives 1 notice at once") is behaviour main already had, so it passes on main; D2 is its guard.
