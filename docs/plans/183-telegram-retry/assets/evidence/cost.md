# Cost (plan 183, AC6; AGENTS.md § Resource cost)

## Steady state, by loop

| loop | interval | work per tick | launches | scaling |
|---|---|---|---|---|
| telegram outbound | 250 ms | one in-process SQLite claim (`CLAIM_NEXT_SQL`); HTTP only when a job is due | 0 | per chat, not per seat; no per-seat work |
| telegram inbound | 250 ms tick, pass = one `getUpdates` long poll (1 s) | ~1 HTTP call/s, as before the split | 0 | none |
| staged-copy sweep | first outbound pass, then every 6 h | one `read_dir` + `metadata` per staged copy, in process | 0 | number of failed copies (≤ 10 per failed send, 7-day retention) |

A job that keeps failing: at most `MAX_ATTEMPTS` = 10 HTTP calls in all, at most 5 in its first minute
(waits 2, 4, 8, 16, 32 s), then about one per backoff, then it fails terminally. Before plan 183: one call
per 250 ms tick, for ever (and each could be a 50 MB upload).

## Simulated hour (fake clock, fake Telegram), branch

`cargo test -p pij-sidecars --lib a_failing_job_costs -- --nocapture`

```
calls per simulated minute: [5, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, ...0] (total 10)
```

Same test on fd7d3bb (red-on-fd7d3bb.md): `left: 14400  right: 10`.

## The FIFO claim rides an index (5,000-row store, ANALYZEd)

`cargo test -p pij-store --test queue_retry_order the_fifo_claim -- --nocapture` explains the production
`CLAIM_NEXT_SQL` itself, not a copy:

```
SEARCH jobs USING INDEX jobs_claimable (state=? AND kind=?)
LIST SUBQUERY 1
SCAN json_each VIRTUAL TABLE INDEX 1:
LIST SUBQUERY 2
SEARCH jobs USING INDEX jobs_claimable (state=?)
CORRELATED SCALAR SUBQUERY 3
SEARCH earlier USING INDEX jobs_serial (serial_key=? AND state=? AND rowid<?)
USE TEMP B-TREE FOR ORDER BY
```

Existing index `jobs_serial (serial_key, state)` (migration 0002); no new index, no migration.

## Launches: live fleet reading BEFORE (machine daemon, read-only ping)

```
1793176880  spawns.total 497085
1793176912  spawns.total 497540   (+455 in 32 s, all existing sites; no telegram op)
telegram/sidecar ops: {"ps.sidecar-process-group":1}  (background runner, unrelated)
```

The Telegram worker has no process launch site before or after this change (no `Command` in
`crates/sidecars/src/telegram*.rs`); `crates/testkit/tests/spawn_sites.rs` is unchanged and green.
AFTER reading: the prime's post-merge live reading (not run by the coder: the machine daemon is read-only
for this seat and runs main).
