# Plan 208 evidence: directory polish

Phase `ph-00d0`, written by pij-local-warbler (coder), 2026-11-06. Script [`private-run.sh`](private-run.sh), transcript [`private-run.txt`](private-run.txt), with the host name shortened to `<host>`.

Local cargo ran only through `nice -n 19 taskpolicy -b just shared-build` into the lane target, scoped with `-p`. The build backoff of 06 Nov 11:03 to 12:22 made CI the test proof for most of the phase. TS tests are CI-only (C25).

## Gates

| Gate | Result |
|---|---|
| CI on 6e40ada (run 42153743796): `check (22)`, `check (24)`, `rust-lint`, `rust-test` | all pass; `shared-build.test.ts` 15 tests on both Node versions |
| CI on 69e9549, the Rust changes' last head | all pass |
| `test -p pij-testkit --test spawn_sites` (local) | green, no new launch site |
| new tests | `holders`: `a_collision_moves_from_on_by_a_millisecond_without_the_wall_clock`, `times_parse_with_a_zone_or_as_a_utc_date_and_render_locally`; `directory_http.rs`: `a_batch_assign_is_one_commit_all_or_nothing`, `the_prime_records_a_closed_seats_past_holding`, `review::review_reassign_keeps_both_holdings` (replaces the same-second test); `shared-build.test.ts`: owner names the seat, cargo runs niced and in background QoS |
| `ddocs-check`, `local-path-check` (main checkout's tsx) | clean |
| `biome check` on changed TS | clean |

## F003 under load (bp-0003)

`review::review_reassign_keeps_both_holdings` from the `directory_http` test binary built at 6e40ada, run alone 50 times in a row under `nice -n 19 taskpolicy -b` (lowest priority, efficiency cores) while the fleet worked: **50 of 50 passed** in 486 s, load average 16.2 at the start and 19.1 at the end. The test no longer needs the assign, release and reassign to fit in one wall-clock second: `fresh_from_at` takes the clock as an argument, and the unit test drives the collision with a fixed one.

The instrument was checked first: an earlier loop with a wrong test filter matched 0 tests and the loop counted all 50 as failures, so a pass here means the test ran (`1 passed; 21 filtered out` in each log).

## Cost (bp-0008, bp-0010)

Private daemon from this branch (debug `pij-rs` at 6e40ada, real ddocs), temp HOME and state, private `-L` tmux server, port 47208, scratch repos, four scratch holders. Launch deltas from `pij-rs --json ping` (`data.spawns.by_op`), same daemon pid throughout:

| Step | Launches | By op |
|---|---|---|
| at rest, 60 s apart | 0 | |
| `directory assign delivery h1` (first holding, window renamed) | 10 | git 7, ddocs 2, tmux rename 1 (unchanged from plan 195) |
| `directory assign storage h1` (second holding, rename skipped by the F17 rule) | 9 | git 7, ddocs 2, no rename |
| `directory assign --batch` of 3 holdings | 12 | git 7, ddocs 2, tmux rename 3: **one commit** (`community: assign 3 holdings`); three single assigns would be 30 |
| `directory lineage add` (closed seat, zoned `--to`) | 9 | git 7, ddocs 2: one commit; stored as UTC (`2026-10-02T12:00:00+10:00` became `2026-10-02T02:00:00.000Z`) |

Window names after the run: `fleet:delivery` for h1 (its first holding kept), `fleet:coder` for h2 to h4.

No timer, poll or sweep is added. The writer's lock retry (F18) runs only when a foreign `index.lock` is found and is bounded by the existing retry; the marker file is one in-process write per commit, no launch.

`just shared-build` (bp-0010): each cargo command now launches `/usr/bin/nice` and, on macOS, `/usr/sbin/taskpolicy`, each exec-ing into the next, so one or two extra launches per cargo command and none at rest or per poll. Checked locally with a fake cargo reporting `niceness=19 taskpolicy=-b` for each command of a `--then` batch under a fake taskpolicy, `niceness=19` with none, and a run under the real `/usr/sbin/taskpolicy`.

### Live daemon (before deploy)

The machine daemon (pid 93273, main's build), 06 Nov 13:20:56 to 13:21:56: `data.spawns.total` 14723 to 14797, **74 launches in 60 s (1.23/s)**, against 1.27/s before #130. `just spawn-rate --seconds 20`: 32.4/s sampled machine-wide, `pij-rs daemon` children 0.0/s lower bound. The after-deploy reading is owed at merge.
