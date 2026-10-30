# Review: PR #99 (plan 185, daemon CPU and memory) at `3f02e3f`

Reviewer: `pij-review-pr99`. Read-only. All probes were temporary edits, restored with `git restore <file>`; the worktree is clean.

**Verdict: merge after fixes. 1 blocker, 1 should-fix, 6 nits.**

F1, F3, F4, F6 and the contract poll are correct. F5's policy and brakes are right, and four of its mutants reproduce independently. The blocker is in F2: it drops a final turn for good, even though a revive can lift the tombstone that triggered the drop.

---

## BLOCKER

### B1. F2 drops a source's final turn because a tombstone is "permanent", but resume and revive lift tombstones
`crates/daemon/src/background.rs:1261-1266` (`finish_source`) and `:1326` (`drop_final_turn`).

The code comment says "A tombstone is permanent, so this final turn can never be handed off". That is false:
- `crates/daemon/src/registration.rs:892-895`: a resuming registration sets `tombstoned_at = None` and `tombstone_reason = None`.
- `docs/how/pij-rs-api.md:130`: "A lift clears both tombstone fields, keeps the address…".
- The death obituary (`pij-rs-api.md:876`) tells the parent: "revive with `pij-rs revive <id>`".

**Failure scenario.** A seat owns an events source. The death sweep tombstones the seat (`observed-dead`). Its source ends. Within 100 ms the branch writes `bg.final-turn-dropped` and runs `mark_notified`. The parent then revives the seat, as the obituary invites. The revived seat never gets the final turn or the events since the last batch.

On main the same turn waited, with the error "its final turn waits", and was delivered after the lift. Main's real bug was only the re-publish on every tick. This is a delivery-behaviour change, against ruling **R4** ("bg completions and deliveries must behave identically").

**Evidence.** I added a temporary test: tombstone the owner, tick three times, lift the tombstone, tick three times.
```
$ cargo test -p pij-daemon --lib -- background::            # on 3f02e3f + probe test
assertion `left == right` failed: the revived owner gets its final turn
  left: 0
 right: 1
test result: FAILED. 83 passed; 1 failed
```
Then I kept the once-guard and replaced the drop branch with main's refusal ("its final turn waits"):
```
test background::tests::review99_a_revived_owner_still_gets_its_sources_final_turn ... ok   (bg.killed count == 1: no flood)
test background::tests::a_retried_completion_publishes_its_event_once ... ok
test background::tests::every_new_job_still_publishes_its_own_completion ... ok
test background::tests::a_tombstoned_owners_source_ends_once_with_its_reason ... FAILED      (it asserts the drop)
```

**Fix.** Delete the drop branch, `drop_final_turn`, `FINAL_TURN_DROPPED` and the test that asserts the drop. The once-guard and `RepeatedFailureLog` already stop the flood: the spine row and the 10 log lines a second.

A waiting source does not block other jobs. `tick()` collects `first_error` and `continue`s for each job (`background.rs:642-691`).

The waiting source still costs 2 in-process sqlite reads per 100 ms tick (`registry.get` plus the once-guard lookup, which hits on the first row it checks). It launches nothing. The cost table needs that as a row.

**Policy or brake.** Ending a pending turn because its owner is tombstoned is a **policy**: removing the check changes *what* happens, not how much. It needs a prime ruling before it ships. If you want it, tie it to something revive cannot undo, or re-deliver on resume.

---

## SHOULD-FIX

### S1. The "once per boot" sweep runs on every 600 ms tick while any pane keeps failing
`crates/daemon/src/pane_observer.rs:208` runs the sweep under `reconcile_world`. `world_reconciled` is set only on a pass with no errors at all (`:324-329`).

**Failure scenario.** One pane's drain or capture keeps failing, for example its sink is missing or a capture keeps erroring. The world is then never reconciled, and on every tick the sweep:
- runs a `list-panes -a` round trip,
- `stat`s every owned, non-live entry,
- does an unconditional `ledger.rewrite` (write plus rename of `.owners/default.list`, a Defender-inspected create every 600 ms),
- prints the attribution line whenever anything is in the grace window (up to 5 minutes of log lines every 600 ms).

That contradicts the PR body, `daemon-cost.md` ("Once per boot") and the test name `the_orphan_sweep_runs_once_per_boot_not_per_tick`, which only covers the happy path.

**Evidence.** A temporary test with `FakeTmux::script_tap_error` on three ticks:
```
assertion `left == right` failed: errors, sweeps
  left: (3, 3)
 right: (3, 1)
```
**Fix.** Give the sweep its own once-flag, set after its first successful attempt, separate from `world_reconciled`. Also skip `rewrite` when the ledger did not change.

---

## NITS

- **N1. Unwitnessed veto in the sweep:** the process-local `taps` keys chained into `live` (`crates/tmux/src/lib.rs`, `sweep_orphan_taps`). I replaced them with `.take(0)` and all of `cargo test -p pij-tmux` stayed green (22 + 33 + 15 + … passed). It is a **brake**: removing it removes the same set or more. It is mostly redundant at boot, because the tap map is empty before the first attach, but it matters with S1. Add a witness or a comment saying it is belt-and-braces.
- **N2. Unwitnessed settle guard:** `MTIME_SETTLE` (`crates/sidecars/src/background.rs`, `records_to_observe`). Replacing `settled.then_some(mtime)` with `Some(mtime)` leaves `cargo test -p pij-sidecars` green (62 + 9 passed). Coarse mtimes are hard to fake, so record it as a tripwire, not a proof. Mutating the mtime short-circuit (`if true {`) does go red, as M8 says.
- **N3. Duplicate head read with a panic:** `SqliteSpine::newest` (`crates/store/src/spine.rs:179`) duplicates `SqliteOrchestration::spine_head` (`:116-124`, `COALESCE(MAX(seq),0)` plus `sql_u64` returning an error). The new one uses `expect("spine sequences are positive")`, which panics in the daemon on a corrupt row. Reuse `sql_u64`.
- **N4. "One indexed seek" overstates the once-guard:** `background.rs` notify. The query is an index range scan on `(seat, kind)` with `json_extract` per row until a match. On a miss it reads every row of that seat and kind. Scratch sqlite with 300k rows: `SEARCH spine_events USING INDEX spine_by_seat_kind (seat=? AND kind=?)`, 0.156 s per miss, so about 1.2 s on the live 2.4M-row owner. It happens once per job, so the cost is fine; fix the comment.
- **N5. `std::thread::sleep` inside an async contract:** `crates/testkit/src/contract.rs` (D-185-4 poll). It blocks the runtime thread for 20 ms per retry. Use `tokio::time::sleep`. Otherwise the poll is correct: bounded at 5 s, and the `assert!` after the loop still fails when the echo never arrives.
- **N6. Two doc claims slightly too strong:**
  - (a) "a default-server daemon never removes a `-L` server's sink, even in a shared tap root" (PR body, `daemon-cost.md`, `ledger.rs`). Sink names are only pane ids, so in a shared root `%5` on both servers is the same `_5.raw` in both ledgers. Ownership by name cannot separate them. Production can't hit this, because the tap root is `<state_dir>/pane-signals` (`daemon/src/lib.rs:860`), but say "distinct state dirs".
  - (b) Trim: "the burst's later writes still show the pane active" assumes a burst of more than one write. A burst of a single write, such as one keystroke echo, can vanish whole in the one-syscall window. That is acceptable; say so.

---

## Focus answers

- **F1, cursor attach: correct.** `subscribe` still creates the live receiver before reading `newest`, refuses `since > newest`, and replays `tail(None, since)`. The seq-based dedupe between the replay and live legs is unchanged, so an event appended during the attach shows up once.
  - Cursor at 0: a full cold replay, as before.
  - Cursor at the head: an empty replay.
  - Cursor beyond the head: `CursorBeyondSpine` with the right `newest`.
  - `MAX(seq)` equals the last row of the old ordered `tail`.
  - Every wrapper delegates `newest` (EventBus, CountingSpine, test spines).
- **F2: see B1.** The once-guard is keyed on `(owner, kind, msg_id)`, and the msg_id is `pij-bg:<job_id>:finished`. The job id is `bg-<ms>-<128-bit random>` (`background.rs:328`), so no legitimate job collides. Retries and restarts re-run the guard and find their own row. Every legitimate job's completion *event* is still published. The final *turn* of a source whose owner is tombstoned is the thing lost.
- **F3: correct.** Running records are re-read on every pass, so state changes such as a cancel are seen. New records move the directory mtime. `persist` writes `*.json.tmp` and renames it into the directory, and `.tmp` files are skipped. The mtime is read before the listing. An mtime in the future fails `duration_since`, so the directory is re-listed every pass (the safe direction).
- **F5:**
  - **Policy:** pane absent from the adapter's own server's `list-panes -a`. M-A (ignore liveness) goes red.
  - **Brakes:** the mtime grace (M-B, red); the process-local tap veto (N1, unwitnessed); unreadable or future mtime treated as recent (in the code).
  - **Ownership checks:** sinks named in other ledgers are never claimed (M-C, red); `-L` adapters never claim (M-E, red; it is M11).
  - Your four proofs:
    - **A `-L` adapter never claims or deletes another server's taps:** yes. Test plus M-E.
    - **A live pane's legacy tap survives:** yes. `live.exists()` in the claim test, plus M-A.
    - **A file younger than 5 minutes survives:** yes. Plus M-B.
    - **A concurrent second daemon or a private `-L` server can't delete the default server's live taps:** yes in practice, because each daemon's tap root lives in its own state dir. A shared root has the N6a caveat.
  - **Ledger lost-update race:** `record` and `rewrite` are not atomic with each other. The only production caller of `attach_pane_tap` is the observer, which runs in sequence with the sweep (`pane_observer.rs:229`), so the race can't happen. If it did, the result would be a leak, never a wrong deletion.
- **F6: correct.** Every `Delivered` receipt is keyed on the recipient:
  - `delivery/mod.rs:854` and `:1372` use `msg.to`,
  - `:2175` and `:2205` use the recipient,
  - `pointer/worker.rs:385` uses `msg.to`.
  - The sync dispatch path already relies on `latest_matching_message(&target.id, "delivery.outcome", …)` (`governance.rs:1783`).
  - The keys that differ are federation outcomes, keyed on the *sender*, and parking refusals. Dispatch targets are local `SeatDescriptor`s, and refusals don't count.
  - Paging is the existing keyset-paged `matching_since` over `(at, seq)`.
  - One difference from main: the first `Delivered` per msg_id is now the smallest `(at, seq)` instead of the smallest `seq`. It only matters if one msg_id has two Delivered receipts with `at` out of order. Harmless.
- **Contract poll (D-185-4):** bounded, and still fails when the echo never arrives (N5 is style only).
- **Cost:** the diff adds no `Command::new`, `spawns::record`, `.spawn()`, interval or `TickLoop` (grep of the `+` lines comes back empty). The loops touched are as the PR states, apart from S1's conditional per-tick sweep and B1-fix's 2 reads per stranded source per tick.
  - The replay numbers are believable. The attach was a full decode of 3M rows (about 1.1 GB of payload into `Vec<Event>`), and `MAX(seq)` on the rowid is a b-tree seek.
  - I did not re-measure them.

## Mutants I ran myself (each restored afterwards)

| mutant | result |
|---|---|
| M-A sweep ignores `live` (policy) | `the_claiming_sweep_removes_only_old_sinks_of_absent_panes` FAILED |
| M-B no mtime grace (brake) | same test FAILED |
| M-C claim ignores other ledgers | same test FAILED |
| M-D drop the process-local taps veto | **all pij-tmux green** (N1) |
| M-E `for_server` claims legacy sinks | `a_named_server_adapter_never_claims_or_sweeps_a_pre_ledger_sink` FAILED |
| M-F no `MTIME_SETTLE` | **all pij-sidecars green** (N2) |
| M-G sidecar lists every pass | `bg_worker_never_reopens_…` FAILED |

Baselines before mutating: pij-tmux sweep, trim and stat tests 4/4 on real private `-L` tmux. pij-daemon lib (`background`, `lifecycle`, `pane_observer`, `governance`) 83 passed apart from my probe. pij-sidecars `bg_` 2/2.

## Not run
- `just rust-check` and `harness checks` on the whole workspace: the disk was at 98% and the packet asks for scoped builds. I ran `-p pij-daemon --lib` (filtered), `-p pij-tmux` and `-p pij-sidecars`. My target dir is 3.0 GB and 40 Gi is free.
- The TS stages: no `node_modules`, and installing is forbidden.
- The private-daemon replay and the 3M-row attach bench: I did not seed a large store. I used a 300k-row scratch sqlite only for the N4 query plan, and deleted it.
- The live `pij-rs --json ping`: I needed nothing from the live daemon. The cost claim was checked by reading the code (no new launch sites).
- The events-bus concurrency tests: I relied on the PR's M1/M2 receipts and on reading the code (the subscribe ordering is unchanged).
