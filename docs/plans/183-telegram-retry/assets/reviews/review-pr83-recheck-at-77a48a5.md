# Recheck: PR #83 at 77a48a5 — seat pij-review-pr83

**Verdict: merge. 0 blockers.** B1 and S1 are fixed at one seam, and N1–N4 are done as claimed. Two follow-ups remain, neither of which blocks:

- **W1:** three backoff guards in N2/N4 have no test that pins them.
- **R1:** a narrow race lets the worker delete a copy that is about to become live again.

## 1. B1/S1 at one seam

- **Both of my probes now pass** (temporarily re-applied from `review-83-probes.patch`, then removed):
  ```
  probe 1 (conflict on #2): 400 … live m#1 copy exists: true / live m#2 copy exists: true
  probe 2 (staging refusal): 400 … live #1 copy exists: true
  test result: ok. 2 passed
  ```
- **Every `remove_file` on staged copies goes through the seam**, except the worker's delivered copy (below) and `.partial-*` (the request's own temp file). I grepped `remove_file` across `crates/sidecars` and `crates/daemon/src`:
  - `release_staged` brakes on `LiveCopies`, and so does `sweep_staged`;
  - the sweep skips when the live read fails;
  - the default `Queue::live_payloads` refuses, and the only non-SQLite impls are test fakes (`FakeQueue` implements it; `CountingQueue` and `FailFirstDeliveryAck` are test-only).
- **Delivered copy (`telegram.rs:979`): the coder's argument is true at the instant of removal, not across the producer's window.** The live-row unique index means at most one live row per `kind` + `msg_id#n`, so no *other* live row shares the path while it is removed. The gap is that `TELEGRAM_STAGING` (daemon-side) does not cover the worker. **R1 (nit)** is this race:
  1. the producer holds the turn and reads live, which includes `P` (running `m#1`);
  2. it stages an identical resend, renaming over `P`;
  3. the worker finishes, removes `P` and acks Done;
  4. the producer's `enqueue_all` now inserts a FRESH `m#1` that points at the deleted `P`, and that row fails terminally ("unreadable").

  The bytes were delivered once, so nothing is lost. The cost is a misleading "queued" receipt plus a spurious failure event. The trigger is an identical retry that lands while the original upload completes; a CLI retry after a slow first request is the realistic case.

  The sweep has the same shape over a microsecond window: its metadata read, then a rename, then its `remove`. That needs a resend under a msg_id whose identical copy failed more than 7 days ago. Fix options: after `enqueue_all`, check that fresh rows' staged paths exist (re-stage, or refuse honestly), or move the turn into sidecars and take it in the worker's removal and in the sweep. Not merge-blocking.

## 2. N2 supervision

- **Correct.**
  - Each pass runs in its own spawned task.
  - A panic leads to a `telegram.loop-restarted` event on `pij-telegram-loops`, then a sleep of 1 s doubling to a 60 s cap.
  - The shift overflow saturates to the cap.
  - Poisoned mutexes are recovered.
  - A job that panics mid-pass stays `running` until the 300 s lease fails it, so a poison job cannot panic-loop.
- **Flood bound as written:** at most about 1 event per minute per loop in steady state, so no flood.
- **W1 (should-fix, test gap): nothing pins the backoff.** My injections, all restored, all survived with exit 0:

  | mutation | result |
  |---|---|
  | R1 restart sleep → `Duration::ZERO` | survived: 62 + 8 passed |
  | R2 `RESTART_CAP` removed | survived |
  | R3 `INBOUND_RETRY_CAP` removed | survived |

  - **R1** is the flood case: panics then repeat every 250 ms tick, which is 4 spine events a second. The event still reports a growing `backoff_ms`, so it is a config echo wearing a measurement name; the restart test asserts that number, never the wait.
  - **R2/R3** are the opposite failure: without the caps the waits grow without bound (2²⁰ s is about 12 days), so after a long outage inbound stays silent long after Telegram is back. That is Jordan's control channel.

  Fix: one paused-clock test (`tokio::time::pause`) per guard, asserting the gap between passes and its cap.

## 3. N3, N4

- **N3:** `TelegramLoops::shutdown` signals both loops, then joins (`signal()` returns the JoinHandle). The coder's N3 mutant kills it.
- **N4:** inbound failures wait 1 s doubling to 60 s; waiting passes return `Ok` and do no HTTP. The coder's N4 mutant kills that; the cap itself is unwitnessed (R3 above).

## 4. Cost

- No launch sites added: no `Command` in the diff.
- `live_payloads` is indexed. `EXPLAIN QUERY PLAN` on a database migrated from `crates/store/migrations`:
  `SEARCH jobs USING INDEX jobs_claimable (state=? AND kind=?)`
- It is bounded by the number of live file rows, and runs once per file send and once per 6 h sweep, never per tick.
- Inbound waiting passes cost one mutex check per 250 ms.

## Run

- `cargo test -p pij-sidecars`: 62 + 8 passed.
- `-p pij-store` (`queue_retry_order`, `queue_enqueue_all`): 4 + 3 passed.
- `-p pij-daemon` (`telegram_order`, `shim_http`): 2 + 26 passed.

## Not run

`harness checks`, clippy and fmt (CI 40663843738 is cited). No live daemon, no real Telegram.
