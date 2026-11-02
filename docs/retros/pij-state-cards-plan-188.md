# pij-state-cards: fast seat cards (plan 188)

**magicWand:** a `Spine` read that cannot be written unbounded. All three slow reads
were one innocent-looking call, `tail(Some(seat), Seq(0))`, and its SQL looked
seat-scoped (`seat = ?2`) while its `?2 IS NULL OR` made it a full scan. Plans
185 and 187 each removed one caller of the same shape. The port still offers
"every row after a cursor" to any hot path. A clippy-style lint, or a port in
which the seat-scoped forms carry a mandatory kind (as `latest_matching` and
`all_matching` now do), would make the fourth instance impossible rather than
found.

**What shipped:**
- Card and note: one newest-of-a-kind seek each (`latest_matching`).
- Held deliveries: a new required `Spine::all_matching(seat, kinds)`, `INDEXED BY
  spine_by_seat_kind`. Without the hint SQLite picks `spine_by_seat` to avoid a
  sort and reads every row the seat has (0.92 s for the 3.59M-row seat).
- `SqliteSpine::tail`'s seat form is its own statement on `spine_by_seat`.
- `GET /v1/cards`, built by the same function as `/v1/state`.
- `stateNoteAt` / `stateNoteSeq`.
- Single card 1.3–2 s → 2–5 ms; the 3.59M-row seat 13 s → 5 ms; 86 cards in one
  ~55 ms call. 1,893/1,893 cards identical to main on a live-shaped seed.

**difficulties:**
- **"Concurrent calls serialise" was a symptom, not a lock.** The 4-connection
  pool and the EventBus wrapper were both clean. A `sample` showed each sqlx worker
  busy only ~25% of the time. What settled it was four `sqlite3` processes outside
  the daemon: they slowed from 0.43 s to 1.40 s each. Four full reads of a 2.3 GB
  file compete for memory bandwidth. Worth encoding: when "serialised" is the
  symptom, run the external-process control before hunting for a mutex.
- **A shared `CARGO_TARGET_DIR` across two source trees silently mixes them.** I
  built a `git archive` of main against this worktree's target to get red-on-main
  receipts, and cargo reused the branch's `pij-core` (workspace-relative hashing).
  The errors were confusing ("missing `all_matching`" on main). A separate target
  dir fixed it. `stretch:` a `just red-on <sha> <test files…>` recipe that does the
  archive, the separate target and the copy-in, because every plan's AC6 needs it.
- **Load-induced test timeouts.** With the fleet at load average 20–27, 14 tests
  timed out on 1 s process probes and pool waits. Each passed when re-run alone.
  `cargo test` without `--no-fail-fast` also hid every target after the first red
  one, and I almost missed the real failure (the corpus manifest) behind them.
- **The main-side golden took 46 minutes** (1,893 seats × 1.3–3 s, slower under my
  own compiles), against 40 s on the branch. Copying the seed (`.backup`, 4 s) to
  run the branch side in parallel was the right call. Check first that the main
  daemon's boot wrote no rows, so that the two copies are the same input.
- **The bulk card needs session status per seat.** On the seed every session is
  nulled, so `/v1/cards` timing excludes transcript reads. On the live fleet each
  bound seat adds its session-status read (as `/v1/state` already does). The
  prime's deployed reading is the real number.

**Encoded:** the store plan tests (`a_seat_tail_seeks…`, `all_matching_seeks…`)
fail if any future edit puts the `OR` back or drops the index hint. The
counting-spine card tests fail if any card read calls `tail`. The state-card
golden pins the wire shape.
