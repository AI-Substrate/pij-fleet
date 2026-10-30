# Plan 187 diagnosis: the lagging-cursor loop is real; the log spam is not

Taken 2026-10-30 18:20–18:35 AEST. Live daemon pid 63433 (main `32b136e`, booted 18:11:49) read-only (R2): `daemon.log`, `sqlite3 -readonly ~/.pij-rs/pij.sqlite`. Everything else ran on a private daemon (R3) with [`private-replay.sh`](private-replay.sh) and a client that copies chainglass's loop, [`chainglass-like.mjs`](chainglass-like.mjs).

## H1 (ac-0001): proven. A client behind the backlog never gets a frame

**The seed.** One private store whose `spine_events` copies the live spine's `seq, v, at, kind, seat` (read-only `ATTACH`) with a synthetic payload of the same length: **4,157,703 rows, 1.54 GB of payload**, the live layout exactly, and no seats, jobs or registry rows.

**The live backlog's shape** (`sqlite3 -readonly`, live store):

| kind | rows | seq range |
|---|---:|---|
| `bg.lost` | 3,585,016 | 30,301 – 4,157,629 |
| `seat.put` | 262,000 | 1 – 4,157,672 |
| everything else | ~310,000 | |

chainglass advances its resume cursor only on `seat.put`, `seat.tombstone`, `role-set`, `report.now` and `report.state`. The longest runs of the spine with **none** of those kinds are 291,544 rows (990,516 → 1,282,060), 239,110, 230,653, 215,048 and 180,397. A client crosses such a run only if it receives all of it inside one 5 s connection, or its cursor stays at the run's start.

**The client.** `chainglass-like.mjs` copies `rs-event-stream.ts` and `rs-client.ts` (read, not imported, R1): 5 s recycle, a fresh process at cursor 0, cursor advanced only on the five resume kinds, the same NDJSON parse.

**Main `32b136e` on the seed, 60 s** ([`private-main.txt`](private-main.txt), [`private-main-client.jsonl`](private-main-client.jsonl)):

| | |
|---|---|
| connections | 12 |
| frames received, including `hello` | **0** on every connection |
| cursor | 0 → 0 |
| daemon CPU | 131–245 %, about 220 % steady (CPU time per 5 s) |
| daemon RSS | 16 MB idle → 6.0 GB at +6 s → **13.3 GB at +64 s**, still rising |

One uncut attach on the same seed ([`ttfb-main.txt`](ttfb-main.txt)): first byte after **5.61 s** from cursor 0 and 1.45 s from cursor 3,000,000.

**Why.** `http::events` awaits `EventBus::subscribe` before it builds the response, and `subscribe` runs `Spine::tail(None, since)`: one `fetch_all` of every row after the cursor, decoded into a `VecDeque`. The `hello` frame is chained after that. From cursor 0 that is 5.6 s, longer than chainglass's 5 s recycle, so the client aborts before it sees a byte, its cursor never moves, and the next attach starts the same read. Aborting does not stop the read already in flight: RSS keeps rising across connections because each abandoned 4M-row read finishes anyway while the next one starts. That is the CPU and the RSS swings in the live sample (`sample-live.txt`: `EventBus::subscribe` → `SqliteSpine::tail`, the sqlx workers and `SqliteRow` drop).

Plan 185 F1 fixed the *head* lookup (`Spine::newest`), not this replay.

**The fix has to beat a second bar, not only the first frame.** A fast first frame is not enough for chainglass to converge: it must also receive a whole resume-free run (up to 291,544 rows) within one 5 s connection, about 60,000 rows a second end to end. The paged replay is measured against that in ac-0003.

## H2 (ac-0004): refuted. The refusal is logged once on the live daemon

The plan's "439 lines in 8 minutes" is the per-minute count for minute `08:11` UTC, and the daemon that wrote those lines is the one *before* the bounce:

```
$ tail -c 2000000 ~/.pij-rs/daemon.log | grep "background reconciliation failed" | cut -c1-16 | uniq -c | tail -3
 595 2026-10-30T08:09
 571 2026-10-30T08:10
 439 2026-10-30T08:11
```

Pre-bounce the line came ten times a second (~575 a minute; the last one 08:11:48.554). Pid 63433 started listening at 08:11:49.196, logged the refusal **once** at 08:11:49.298, and `daemon.log` has not been written since (mtime 18:11 local, 751,175,352 bytes, read at 18:23). `RepeatedFailureLog` (plan 185) holds on the live path.

What it does not do is what ac-0004 asks literally: it remembers the tick's *first* error text, not each source. `BackgroundService::tick` returns only `first_error`, so (a) a second stranded source is never logged while the first fails, and (b) any other failure that becomes the tick's first error, even once, makes the stranded source's line log again afterwards. Neither is happening live.
