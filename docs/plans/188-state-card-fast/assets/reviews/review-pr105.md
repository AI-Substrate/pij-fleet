# Review: PR #105 (plan 188, fast seat cards) — pij-review-pr105

Head `96421ee`, diff `origin/main...HEAD`. Read-only; no commits, pushes or comments.

**Verdict: merge after fixes.** Blockers: **0**. Should-fix: 3. Nits: 4.

The core change is correct. The seat form of `tail` asks the same predicate as main (`seq > since AND seat = ?`, `ORDER BY seq`), split out of the `OR` so SQLite can seek `spine_by_seat`. Every production caller gets the same rows in the same order. The card and the note pick the same record main picked. `INDEXED BY` cannot reach a store without its index. The findings are gaps in what the tests and receipts prove, plus one blast-radius issue in the bulk route.

---

## Correctness

### `tail(Some(seat))` callers: same rows, same order ✅
Swept with `rg --hidden -n "\.tail\("` (and the multi-line form). Every pointer-worker, registration and delivery hit is inside a `#[cfg(test)]` module (worker.rs:983, registration.rs:1796, delivery/mod.rs:2608). The production seat-form callers:

| caller | since | effect |
|---|---|---|
| `http/governance.rs:230` (`tail(subject, Seq(0))`, idempotent-append search) | 0 | identical predicate, `ORDER BY seq` |
| `http/governance.rs:1987` (`tail(Some(&dispatch.to), Seq(0))`) | 0 | identical |
| `http/mod.rs:2244` held-release replay (`tail(Some(seat), held.seq)`) | the hold's seq | identical; filters `delivery.outcome` |
| `EventBus::tail` (`events/mod.rs:287`) | pass-through | — |

- Old SQL: `WHERE seq > ?1 AND (?2 IS NULL OR seat = ?2) ORDER BY seq`. New: `WHERE seat = ?1 AND seq > ?2 ORDER BY seq` for `Some`, and `WHERE seq > ?1 ORDER BY seq` for `None`. The row sets are the same, and so is the total order: `seq` is the unique rowid, so ties are impossible.
- An empty seat returns `[]` either way. A NULL-seat row never matches `seat = ?` in either form.
- Governance still reads every row the seat has (it is now a seek, but still unbounded). That is outside this PR's scope.

### Card and note: same record as main ✅
- Main's `tail(Some(seat), Seq(0)).rev().find(kind)` returns the highest-seq row of that kind. `latest_matching` runs `ORDER BY seq DESC LIMIT 1` (spine.rs:290), so it returns the same row.
- **Card and note in the same tick:** they are different kinds read by two independent lookups, each by seq. `at` plays no part in either.
- **Cleared note:** `clear` appends a new `report.state` with `note: None`, which becomes the latest. `stateNote`, `stateNoteAt` and `stateNoteSeq` all go null together (http/mod.rs:2947-2953). This is pinned on the real store by `report_http.rs` `the_state_note_carries_its_own_time_and_cursor` (clear step), and by the coder's mutant `note-time-kept-after-clear`.
- `stateNoteAt` and `stateNoteSeq` come from the `report.state` event itself (`latest_state`, report.rs:243-257), never from the card. This is pinned by `assert_ne!(stateNoteSeq, statusSeq)`, with the record written before the card in card_tests and after it in report_http.
- Plan check (sqlite 3.54, 200k-row skewed history, scratch DB): the single-kind form uses `SEARCH … USING COVERING INDEX spine_by_seat_kind (seat=? AND kind=?)`, with or without ANALYZE. No `INDEXED BY` is needed when there is one kind.

### `INDEXED BY` on a missing index ✅ (cannot be reached)
- A missing named index is a hard error (`no such index: spine_by_seat_kind`), reproduced in a scratch DB.
- `spine_by_seat` comes from 0001_init.sql:51 and `spine_by_seat_kind` from 0020_spine_by_seat_kind.sql:2.
- `tail` and `all_matching` both call `require_current_schema` first, which refuses anything except `SCHEMA_VERSION` (30) with `StoreSchemaStale` (migrate.rs:242). So an older migration level refuses before the SQL runs, and a fresh store is migrated by `pij_store::open`.
- Test stores either go through `open` (FreshStore) or are `FakeSpine` (no SQL).
- The TS side never runs these statements. The only TS SQL against `spine_events` is in harness smoke scripts (`harness/scripts/*-smoke.ts`, `step-on-probe.ts`), which run their own queries without `INDEXED BY`.
- The only way in is someone dropping the index by hand. That would turn a slow card into a loud error, which is the right failure.

### `/v1/cards` ✅ with one should-fix (S1)
- **Auth:** behind `require_bearer`. `Endpoint::peer_access` gives `Cards` → `Refused` (mod.rs:385-390), so a peer key gets `E-RS-PEER-SCOPE`. Because `Cards` is in `Endpoint::ALL`, the existing `peer_scope_tests` walk covers it.
- **Scope:** local roster only (`status.read(None)`, no federation fan-in).
- **Tombstones:** excluded, matching `/v1/seats`' default `retain(tombstoned_at.is_none())`. `/v1/state` still serves a tombstoned seat by id, which is unchanged and fine. The coder's mutant `bulk-includes-tombstoned` pins this.
- **Holds:** included. `bulk-drops-held` pins it, and `all_matching` over `delivery.{held,released,outcome}` folds to the same holds as main.
- **Roles:** `join_roles` (one `list_roles`) against `/v1/state`'s `project_seat` (`read_role` per seat). Both set `seat.role` from the same store, so they are equivalent by reading (role.rs:253-274). No test exercises it (see N2).

---

## Findings

### S1 (should-fix) — the 49–64 ms bulk figure was measured with every session nulled; live, 83 of 87 seats do a session-status fold
- **Where:** `docs/how/daemon-cost.md` (Measured, plan 188, row "Every untombstoned seat's card (86)"), `docs/how/pij-rs-api.md` § Every seat's card. The cost lives in `seat_card` → `session_status_block` (http/mod.rs:2808, 2839-2869).
- **What:** `private-cards.sh` nulls `pid, proc_start, pane, harness_session` on every seat (script line ~52). So `session_status_block` returned `Unbound` without calling the source, and liveness was never probed. The live store looks different:
  ```
  $ sqlite3 -readonly ~/.pij-rs/pij.sqlite "select count(*), sum(harness_session is not null) from seats where tombstoned_at is null"
  87|83
  ```
  Each bound card runs a Unisphere transcript fold. A cold fold reads the whole transcript. Measured read-only on the live daemon for my own seat (1.37 MB transcript):
  ```
  $ pij-rs --json state pij-review-pr105 | jq .data.sessionStatus.elapsedMs   # first read
  697
  # then: 52, 0, 0
  ```
- **Failure scenario:** chainglass starts against a freshly bounced daemon and calls `GET /v1/cards` once. That call does 83 cold folds, four at a time, through a `spawn_blocking` read each. For long-lived primes with tens of MB of transcript, it takes seconds, not 50 ms. Nothing bounds a single seat's fold. One slow seat holds a permit, and the whole response waits for the slowest.
- The per-card cost is the same as main's `/v1/state`, so this is not a regression. But the cost docs claim "49–64 ms" and "no cache, so no memory per seat". In fact `/v1/cards` now populates the per-seat Unisphere cursor map (`unisphere/src/lib.rs` `cursors`) for every bound seat. That cache already existed; the bulk route warms it for all of them at once.
- **Fix:**
  1. Re-measure `/v1/cards` cold and warm with sessions bound, or label the row "sessions nulled; session-status folds excluded".
  2. Add the session-status fold to the backpressure cost row: per bound seat, cold = whole transcript, warm = incremental.
  3. Consider bounding the bulk path's per-seat session read, the way the cold-wake guard does, and filling `SessionStatusBlock::Failed`/timeout instead of stalling the response.

### S2 (should-fix) — one bad seat fails the whole `/v1/cards` response
- **Where:** http/mod.rs:2744-2750 (`Ok((_, Err(error))) => return internal("pij cards", error)`).
- **Failure scenario:** any per-seat error turns the whole fleet's response into a 500, with zero cards. Examples: one seat's `report.now` payload fails to decode (`decode(&event.payload, CARD_EVENT_KIND)?`), a liveness adapter error (a `ps` fallback failure for an `Unanswered` pid), or a `delivery_deferrals` read error. Chainglass, which the plan says seeds every card from this call, gets nothing and falls back to 86 single reads (or shows nothing).
- On main the same corrupt row breaks only that seat's `/v1/state`. The bulk route widens a one-seat failure to the whole fleet.
- **Fix:** put a per-card error slot in the projection (e.g. `{id, error}`, or a sibling `unavailable: [...]` like `FederatedRoster.unavailable`). Or decide explicitly that all-or-nothing is the contract and document it in pij-rs-api.md. Either is fine; right now it is implicit.

### S3 (should-fix) — the PR moved the card's "latest by sequence, not timestamp" guarantee onto a query no test pins
- **Where:** spine.rs:290 (`latest_matching`'s `ORDER BY seq DESC LIMIT 1`), now the only ordering behind `ReportService::card` and `latest_state` (report.rs:311-313).
- **What:** pij-rs-api.md says "Freshness follows event **sequence**, not the greatest timestamp: a later committed event can have an earlier supplied clock." On main that rested on `tail`'s `ORDER BY seq`, which the contract pins by order. It now rests on `latest_matching`, and every event in `spine_contract` has the same `at` (`at: 1_724_800_000_000`, contract.rs:234). So nothing tells "by seq" from "by at".
- **Evidence (reviewer mutant `latest-by-at-not-seq`):** `ORDER BY seq DESC` → `ORDER BY at DESC, seq DESC` survives:
  ```
  -p pij-store --test store_integration: rc=0   31 passed
  -p pij-daemon --test report_http: rc=0        32 passed
  ```
- **Failure scenario:** a later refactor swaps in `spine_by_seat_kind_at` (the natural index for "latest at"), or the clock steps back (NTP). The card then shows an older `report.now` while `statusSeq` and the events stream say otherwise, and every test stays green.
- **Fix:** add one event to `spine_contract` with a later seq and an earlier `at` than its same-kind sibling, and assert `latest_matching` returns the later seq. This is one assert, and it pins the fake too.

### N1 (nit) — the seat form of `tail` is a new statement whose `since` exclusivity no test pins
- **Where:** spine.rs:48-49 `SEAT_TAIL_SQL`.
- **What:** before the split, the seat form shared `seq > ?1` with the unscoped form, which `tail(None, first)` pins. The new copy is pinned only from `Seq(0)`.
- **Evidence (reviewer mutant `seat-tail-inclusive-since`):** `seq > ?2` → `seq >= ?2` survives everything:
  ```
  -p pij-store --test store_integration: rc=0   31 passed
  -p pij-daemon --lib:                   rc=0   667 passed
  -p pij-daemon --test report_http:      rc=0   32 passed
  ```
- No production caller can observe it today. Governance passes `Seq(0)`, and the held-release replay's cursor row is a `delivery.held`, which its `delivery.outcome` filter drops. So it is a nit, but the port doc promises "events after `since`" for both forms.
- **Fix:** add `tail(Some(&seat_a), first)` == `[expected_third]` to `spine_contract`.

### N2 (nit) — bulk/single equality is tested on a fleet that cannot show order or role drift
- **Where:** card_tests.rs `the_bulk_read_is_the_single_read_for_every_seat` (two untombstoned seats, no roles) and report_http.rs `the_bulk_card_read_matches_the_single_read_on_the_real_store` (one seat, no roles).
- **Evidence:** two reviewer mutants survived.
  - **`bulk-skips-roles`** replaces the `join_roles` call with `Ok(())`. Every card's role becomes `None`, and the result is `--lib` 667/667 green, report_http 32/32 green. On the live fleet every PM, prime or PA card would lose its role in `/v1/cards` while `/v1/state` kept it, and chainglass's role column would go blank.
  - **`bulk-completion-order`** fills cards in completion order instead of by index. `card_tests` stayed green. The one `--lib` failure in that run was `http::tests::a_live_idle_claude_frame_under_a_long_stale_working_is_refused` (tests.rs:7592), which never calls `/v1/cards` and passes 3/3 alone on the clean tree, so it was a load flake, not a kill. The docs promise "in seat-id order", and two seats with four permits rarely reorder.
- **Fix:** give one fleet seat a role assertion, and use at least five seats with the first one slowest (e.g. many holds), so a fill in completion order and a missing role join both turn the equality red.
- (Aside: that flake failed once in four full `--lib` runs under `nice` load. It is outside this PR, and worth a line in difficulties.)

### N3 (nit) — `stateNoteAt` is present for an empty-string note
`state_card` keys presence on `note: Some(_)` (http/mod.rs:2947). A `report blocked ""` (if the CLI allows it), after `collapse_whitespace`, gives `stateNote: ""` with a time. That is consistent with `stateNote` itself, so it only matters if consumers treat `""` as absent. I did not check the CLI's empty-note refusal.

### N4 (nit, outside this PR) — a two-kind `latest_matching` still walks the seat's rows
In a scratch DB with ANALYZE, `seat = ? AND kind IN (a, b) ORDER BY seq DESC LIMIT 1` plans as `SEARCH spine_events USING INDEX spine_by_seat (seat=?)`: a reverse walk over every row the seat has until a match. The caller is `publish_pointer_unparked_if_parked` (delivery/mod.rs:2234, kinds parked/unparked) on every reader-read. For a seat with 3.59M `bg.lost` rows and an old park, that is the same shape plan 188 fixed. This PR does not touch it, so it is a follow-up, not a finding against #105.

---

## Golden equality (1,893/1,893): what it proves and what it can't
- **What it compares:** `compare.py` checks main's `/v1/state` against the branch's, for every seat in one seed. It drops the two new keys and then uses whole-dict `==`. So any changed, missing or extra field, nested ones included, would show as a DIFF. It cannot miss a field it can see. It also asserts that `stateNoteAt`/`Seq` are null exactly when `stateNote` is.
- **What it cannot see:**
  - **Liveness, session status and pane are constant on both sides**, because the seed nulls pid, proc, pane and session.
  - **`deliveryDeferrals`, `pendingFyis` and `nativeReceiverReason` are empty on both sides** (no jobs, fyis or leases in the seed). Their code is unchanged, apart from the `?` refactor of error paths.
  - **Holds are thin:** `with_held=1` across 1,893 seats, so the hold fold was compared on one seat.
  - **Only the `/v1/state` path is compared.** It does not exercise `/v1/cards`, the other `tail(Some)` callers (governance, held release), or a `since > 0` read.
- So it is strong evidence for R4 on the card's spine-derived fields. It does not show that the bulk route matches on a live-shaped fleet; that rests on the unit tests and on `seat_card` being shared.

## Cost
- No new loop, poll, timer, launch site or sweep. The new route is request-driven only.
- Per card: the same reads as `/v1/state`, minus the three full-spine walks. Per bulk call: one roster read (1,893 rows, with three correlated indexed subqueries each, tombstoned rows included and filtered afterwards) and one role read, then the per-seat reads, four in flight.
- Liveness is in-process (`kernel_start`), falling back to `ps` only for `Unanswered` pids. That fallback is pre-existing.
- **The session-status fold per bound seat is the dominant unmeasured term** (S1).

## Reviewer mutants
| mutant | result |
|---|---|
| seat-tail-inclusive-since (`>` → `>=`) | **survived** (N1) |
| seat-tail-newest-first (`ORDER BY seq DESC`) | killed: contract.rs:293, store_integration.rs:1219 |
| all-matching-grouped-by-kind (`ORDER BY kind, seq`) | killed: store_integration.rs:807, contract.rs:317 |
| latest-by-at-not-seq | **survived** (S3) |
| bulk-completion-order | **survived** in card_tests and report_http (N2); one unrelated flake in `--lib` |
| bulk-skips-roles (`join_roles` → `Ok(())`) | **survived** (N2) |

Script: `<reviewer scratchpad>/mutants.py`; output: `…/scratchpad/mutants.out`. The tree was restored after each mutant, and `git status` is clean.

Baseline before the mutants: `pij-store` store_integration 31/31, `pij-daemon --lib` 667/667, report_http 32/32, `pij-testkit` all green, card_tests 4/4.

## Not run
- **`harness checks`, `just lint`, `just typecheck`, `just smoke`, and the full workspace `cargo test`:** this was a scoped review, the disk is at 97%, and I built only store, daemon, core and testkit.
- **The coder's `mutants.py`:** not re-run. I read its receipts (`mutants.txt`, 8 mutants, all reported killed) and wrote complementary mutants instead.
- **Any private daemon or seeded store:** `/v1/cards` was not timed. The live daemon runs main, so I could only time `/v1/state` read-only, for the session-status cost.
- **chainglass:** not touched (R1).

## Disk
Worktree `target/` is 6.8G (store, daemon, core and testkit test builds), kept for a recheck. Scratch DB deleted. No seeded store was created.
