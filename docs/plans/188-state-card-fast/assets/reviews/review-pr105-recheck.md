# Recheck: PR #105 at 4d4e98e — pij-review-pr105

**Verdict: merge.** Blockers: **0**. Two new nits (R1, R2), neither gating.

## 1. The four surviving mutants, re-run on the new head (my script, anchors adapted)
| mutant | result |
|---|---|
| seat-tail-inclusive-since | killed: contract.rs:298 |
| latest-by-at-not-seq | killed: contract.rs:405 |
| bulk-completion-order (slot = completion count) | killed: card_tests.rs:431, 478 |
| bulk-skips-roles | killed: card_tests.rs:442 |

I added four new mutants:
| mutant | result |
|---|---|
| bulk-misattributes-task (`tasks.remove` → `pop_first`) | killed: card_tests.rs:431, 478 |
| unavailable-omitted-when-empty (`skip_serializing_if`) | killed: card_tests.rs:446 |
| bulk-unbounded (bulk passes `None`) | killed: card_tests.rs:471 |
| **state-also-bounded** (`/v1/state` passes `Some(200 ms)`) | **survived** in card_tests, report_http and `--lib` (R1) |

The only `--lib` failure under `state-also-bounded` was `a_live_idle_claude_frame_under_a_long_stale_working_is_refused`, at tests.rs:7592. That line is the `/v1/send` status assert, which runs before any state read. It is the same load flake as last round, not a kill.

Clean baseline on 4d4e98e:
- store_integration 31/31, daemon `--lib` 670/670, report_http 33/33, testkit green, unisphere 16/16.
- `cargo clippy -p pij-daemon --all-targets` and `cargo fmt --check` (daemon, store, testkit) are clean.
- The tree was restored after the mutants; `git status` is clean.

## 2. S1: the bound
- **A timed-out read keeps filling the cursor (confirmed by reading and by an existing test).** `UnisphereSessionStatus::status` runs the take → fold → store inside `tokio::spawn` and awaits the `JoinHandle` (unisphere/src/lib.rs:180-236). `tokio::time::timeout` drops only that handle; a dropped `JoinHandle` does not cancel the task. So the fold finishes and writes `*cursor = Some(next)`. This is pinned by `a_cancelled_read_still_keeps_its_cursor` (lib.rs:483-528): it gives up at 0 ms while holding the slot, then waits for `holds_cursor`. That test exists and passes (16/16). I did not mutate it, because the spawn is not a one-token mutant.
- **No permanent starvation.**
  - A call that lands while an earlier fold is still running queues on the seat's slot lock. Its card says `failed: no answer within 200 ms` and leaves one more detached task queued; it never restarts a cold fold.
  - When the cold fold lands, every later read for that seat is incremental from the stored cursor.
  - A seat can only time out on every call if its *incremental* fold (bytes appended since the last read) takes more than 200 ms every time. Even then each call's fold advances the cursor in the background, so the seat is behind, not stuck.
  - Residual: the cursor-kept guarantee is tested only for the Claude fold. Copilot (11 live seats) uses the same wrapper, so it inherits the same detach, but nothing tests it directly.
- **`/v1/state` is unchanged by reading:** `seat_card(&state, seat, None)` (mod.rs:2701), and `None => read.await`. But no test pins it (R1).

## 3. S2: `unavailable`
- **Always present:** a plain `Vec` with no `skip_serializing_if` (mod.rs:2786). Omitting it when empty turns `the_bulk_read_keeps_roster_order_and_roles_on_a_wider_fleet` red.
- **No leak between seats:**
  - Each task builds only its own seat's card and returns that `Result`.
  - The outcome is keyed back by tokio task id to `(index, id)`, including a panicking task via `JoinError::id()`.
  - Misattributing the keying turns two tests red.
  - The error text is the seat's own `PijError` string; the route is local-only.

## 4. Anything broken
Nothing found. Two nits:
- **R1 (nit):** add one card_tests case: a seat whose session answers after 300 ms. `/v1/state` must return `outcome != failed` (or take more than 200 ms), while `/v1/cards` names the timeout. This pins the "only the bulk path is bounded" ruling.
- **R2 (nit, docs):** pij-rs-api.md and daemon-cost.md say the bulk read does the "session-status fold, four seats at a time". After the bound, that is true of the waits but not the folds. A timed-out fold keeps running detached and its permit is released, so a cold first call can have up to every bound seat (83 live) folding at once on the blocking pool. The worst-case response is about ⌈83 ÷ 4⌉ × 200 ms ≈ 4.2 s, not 200 ms. `/v1/seats?sizes=true` already does the same thing (`list_sizes` spawns every seat with no permit at all), so this is no new risk, but the wording overstates the bound. Suggested: "waits bounded at 200 ms per seat, four at a time; a fold that runs over finishes detached".

## Not run
- `harness checks`, smoke, the full workspace tests, and any live or private `/v1/cards` timing (the live daemon runs main).
- The coder's `mutants-review.py`: I read its receipt, which agrees with mine.

`target/` (6.8 GB) deleted after this run.
