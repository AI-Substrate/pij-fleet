# Plan 188 diagnosis: why one /v1/state call takes seconds

Measured 2026-11-02 by pij-state-cards. Base: main `2d9ea28` (release build).

## Setup

Private daemon (R2, R3): [`private-cards.sh`](private-cards.sh) `seed`, then `boot` on port 17882 with its own HOME, state dir and `tmux -L` server. The seed copies the live spine's `seq, v, at, kind, seat` (4,158,818 rows, head 4,158,818; read-only ATTACH). Payloads are padding of the same length, except the five kinds the card decodes (`report.now`, `report.state`, `delivery.held`, `delivery.released`, `delivery.outcome`), which keep their real payload so that every seat has a real card. `seats` (1,893 rows, 86 not tombstoned) and `seat_roles` are copied with `pid`, `proc_start`, `pane` and `harness_session` nulled, so the private daemon has no process, pane or session it could reach. 2.3 GB on disk. The seed has no `sqlite_stat1`, and neither does the live store.

## Timings on main

| call | private seed | live daemon (read-only) |
|---|---|---|
| `/v1/state` pij-state-cards (7 spine rows) | 1.31 s | 1.65 s |
| `/v1/state` pij-far-jackal (2,216 rows) | 1.30 s | |
| `/v1/state` pij-causal-skeram (104k rows) | 1.74 s | |
| `/v1/state` pij-different-smelt (3.59M rows, mostly `bg.lost`) | **15.2 s** | **15.3 s** |
| 4 × pij-far-jackal at once | 6.6–7.0 s each | |

## The reads behind one call, with query plans

Plans: [`plans-main.txt`](plans-main.txt) (EXPLAIN QUERY PLAN plus a `sqlite3 -readonly` timing for each, on the seed).

| # | read | query plan | cost |
|---|---|---|---|
| 1 | `status.read(Some(seat))`: seat row, `last_event_at`, assignment states | SCAN seats (1,893 rows) + indexed correlated subqueries on `spine_by_seat`, `spine_by_seat_kind` | 10 ms |
| 2 | role (`seat_roles` by seat) | SEARCH autoindex | <10 ms |
| 3 | **card**: `ReportService::card` → `latest_event` → `Spine::tail(Some(seat), Seq(0))`, then the newest `report.now` found in Rust | **`SEARCH spine_events USING INTEGER PRIMARY KEY (rowid>?)`**: the whole spine | **0.43–0.62 s** |
| 4 | **state note**: `ReportService::latest_state_record` → the same `tail` | the same full scan | **0.43–0.62 s** |
| 5 | **held deliveries**: `active_holds` → the same `tail` | the same full scan | **0.43–0.62 s** |
| 6 | `delivery_deferrals` (jobs) | SEARCH `jobs_serial` | <10 ms |
| 7 | `pending_fyi_count` (fyis) | SEARCH covering `fyis_pending_by_recipient` | <10 ms |
| 8 | `native_receiver_reason` → registry get by id | SEARCH autoindex | <10 ms |
| – | session status, liveness | file read and an in-process syscall; no SQL, no launch | – |

**Root cause.** `SqliteSpine::tail` filters with `WHERE seq > ?1 AND (?2 IS NULL OR seat = ?2)`. Because of the `?2 IS NULL OR`, SQLite cannot use `spine_by_seat (seat, seq)` and walks the rowid from `seq > 0`, which is all 4.16M rows (2.3 GB of pages) for every seat. One card does that **three times** (reads 3, 4 and 5), which is the 1.3 s. For a seat with many rows, `tail` also decodes and copies every one of its rows into memory, three times, to pick out one or two: pij-different-smelt's 3.59M rows make 15 s.

The indexes the fix needs **already exist** (`spine_by_seat`, `spine_by_seat_kind`, `spine_by_seat_kind_at`). No schema change is needed (R5).

## Why concurrent calls serialise

There is no single connection and no lock: the pool has 4 connections, and the `EventBus` wrapper forwards `tail` without taking its publication lock. A 3-second `sample` of the daemon during 4 concurrent calls ([`sample-main-4.txt`](sample-main-4.txt)) shows the sqlx workers in `sqlite3BtreeNext → readDbPage → pread` and in pcache mutex waits (`pcache1Fetch`/`pcache1Unpin` → `__psynch_mutexwait`), each busy only about a quarter of the time. Control outside the daemon: one `sqlite3 -readonly` process running the read-3 scan takes 0.43 s; four at once take 1.40 s each. So four calls are twelve full reads of a 2.3 GB file competing for memory bandwidth and the page cache: the "queueing" is the full scans, and goes away with them.

## Fix (no schema change)

1. Card and state note: `Spine::latest_matching(seat, [kind])`, the existing port read, `SEARCH spine_by_seat_kind (seat=? AND kind=?)` with `ORDER BY seq DESC LIMIT 1`: 10 ms on the seed.
2. Held deliveries: read only the three delivery kinds for the seat, `INDEXED BY spine_by_seat_kind` (without it the planner picks `spine_by_seat` to avoid a sort and walks all 3.59M of smelt's rows: 0.92 s; with it 10–30 ms).
3. `SqliteSpine::tail(Some(seat), since)`: drop the `OR` so the seat form is `SEARCH spine_by_seat (seat=? AND seq>?)` (20 ms). Same rows, same order. Other callers of the seat form (pointer worker, governance, held release) get faster too.
4. Card content is unchanged: each read still answers "the newest event of this kind for this seat, by seq" and "this seat's delivery events in seq order".
