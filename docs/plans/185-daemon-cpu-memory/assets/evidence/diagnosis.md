# Plan 185 diagnosis: what burns the daemon

Taken 2026-10-29 08:30–08:45 AEST, read-only (R1), on the machine daemon pid 95125 (main `d459dda`, bounced by the prime at 08:26). Tools: `sample` (5 s and 10 s), `vmmap --summary`, `ps -M`, `lsof -iTCP:7461`, read-only `sqlite3 -readonly`, `pij-rs --json ping`, `just spawn-rate`. Busy samples were counted with [`sample-busy.py`](sample-busy.py), which drops the idle leaves (`__psynch_cvwait`, `kevent`, `__recvfrom`, blocking `read`, …) and charges each remaining sample to every distinct frame on its stack.

## Headline

| | |
|---|---|
| CPU | 197–208 % over 60 s (`ps`, five readings 15 s apart) |
| RSS | 8.4–9.1 GB over the same minute; `vmmap`: physical footprint 3.3 G, peak 6.9 G, Malloc Small 6.6 G, 38.2 M live allocations, 1.4 G fragmentation |
| Launches | 0.9 /s from the daemon (`just spawn-rate --seconds 20`). **Launches are not the problem this time.** |
| Defender | `wdavdaemon_unprivileged` 345 %, `wdavdaemon_enterprise` 98 %, `wdavdaemon` 88 %, `epsext` 67 % at the same moment |

10 s sample: 8,909 busy samples ÷ 4,466 samples per thread = **2.0 cores**, which matches `ps`.

| # | Code path | Busy samples (10 s) | Cores | Share |
|---|---|---:|---:|---:|
| D1 | `http::events` → `EventBus::subscribe` → `Spine::tail(None, Seq(0))`, plus the sqlx worker threads running that query | 2,622 + ~2,260 | **1.09** | 55 % |
| D3 | `pij_sidecars::BgWorker::observe_completions` → `read_to_string` → `open()` | 1,658 | **0.37** | 19 % |
| D4 | `PaneObserver` → `TmuxAdapter::drain_pane_tap` → `open()` | 1,054 | **0.24** | 12 % |
| — | `SqliteRegistry::list` (pane observer and others) | 203 | 0.05 | 2 % |
| — | delivery drain | 117 | 0.03 | 1 % |

The 5 s sample agrees on the order (D1 1,074 + sqlite worker 1,004; D3 501; D4 132).

## D1 — every cursor attach decodes the whole spine (CPU and memory)

`EventBus::subscribe` (`crates/daemon/src/events/mod.rs`) finds the newest sequence, to refuse a cursor that is beyond it, like this:

```rust
let newest = self.spine.tail(None, Seq(0)).await?.last()...
```

That is `SELECT … FROM spine_events WHERE seq > 0` with `fetch_all`, then a decode of every row into an `Event`, to read one number. The live spine has **2,978,027 rows and 1.1 GB of payload**. Every `GET /v1/events?since=…` pays it.

Who attaches with a cursor:

- **chainglass** (`next-server`, pid 38110, up 12 h 38 m): a new TCP connection to :7461 every ~3.7 s, one at a time, for the 15 s observed (`lsof`, 30 polls 0.5 s apart). Its `rs-client.ts` `events(from)` sets `since`.
- every OMP/pi seat's `daemon-http.ts` re-attaches with its cursors after a lost stream: after a bounce all of them do it at once.

Recorded-workload replay (`crates/daemon/tests/spine_attach_cost.rs`, release build, synthetic `bg.lost` rows of ~355 bytes, a consumer 10 events behind, `/usr/bin/time -l`), on main `d459dda`:

| spine rows | one attach | peak RSS of the process |
|---:|---:|---:|
| 561,000 (the spine on 2026-10-26, before D2 began) | 0.85–0.93 s | 0.99 GB |
| 2,978,027 (today) | **6.3–7.2 s** | **5.1 GB** |

A consumer that needs 10 events costs 6–7 s of CPU and 5 GB of transient allocation. Two in flight is the 8–11 GB plateau and the churn the prime saw; chainglass alone keeps one in flight continuously. The spine readers in `governance` and `anomalies` also call `tail(None, Seq(0))`, but they are request-driven and did not appear in either sample.

## D2 — one stranded bg source re-publishes `bg.lost` ten times a second (the cliff)

```
sqlite> select kind, count(*), sum(length(payload))/1048576 from spine_events group by kind order by 2 desc limit 3;
bg.lost|2416965|859
seat.put|261395|120
seat.native-resumed|116922|29
```

Of the last 20,000 events, 19,467 are `bg.lost`. Over a 10 s read the spine grew 97 rows, all `bg.lost`, all for job `bg-1792039730998-…` whose owner `pij-different-smelt` is tombstoned. It began 2026-10-26 02:24 UTC.

The cause is in `BackgroundService::notify` (`crates/daemon/src/background.rs`): for an events source it **publishes the completion event first**, then calls `finish_source`, which refuses with "owner … is gone; its final turn waits". `mark_notified` is never reached, so the 100 ms background tick (`lib.rs`, `background_interval`) retries and publishes the event again: **10 events/s, 3.5 KB/s of spine, about 300 MB a day**. The same refusal is written to `daemon.log` ten times a second (507 MB today, #90).

D2 is what turned D1 from about 1 s into 6–7 s: the spine is 5.3× larger than it was before the job ended.

## D3 — the sidecar bg worker opens every record twice per 100 ms tick

`BgWorker::run_once` calls `observe_completions` before and after its claims. Each call is `read_dir` plus `read_to_string` of every `*.json` record, including terminal ones it then skips. There are 3 records in `~/.pij-rs/sidecars/bg`, all terminal since 2026-08-31, so that is about 80 `open()` calls a second for nothing. Nearly all of D3's samples are inside `open()`, not in parsing. On this machine `open()` costs Defender's time: a read-only Python probe opened `live-bg-injected.json` in 3, 9 and 138 ms (stat: about 1 µs).

## D4 — the pane observer opens every tap twice per 600 ms tick

`observe_once` calls `drain_pane_tap` twice for each tracked pane (before and after `capture`). Each call does `File::open`, `metadata`, `seek` and `read_to_end`, even when nothing was appended. With 36 live taps that is about 120 `open()` calls a second. The observer uses only `bytes.is_empty()` from both drains (`tap_active`).

## D5 — tap files on disk

`~/.pij-rs/pane-signals` holds 274 entries and 3.1 GB. **3.19 GB of that is 314 files no daemon has written for more than 10 minutes**: panes from tmux servers that have since restarted (the biggest are `_4663.raw` 383 MB and `_1861.raw` 377 MB, last written 2026-09-15 and 2026-08-31). The 36 live taps hold 3.5 MB, because `attach_pane_tap` truncates on a fresh attach. Nothing in the rs daemon removes a tap file for a pane that no longer exists, and a live tap's file has no bound: `cat >>` appends until the daemon restarts.

## D6: the delivery observer decodes the whole spine at boot (found by the replay)

The private-daemon replay of F1–F5 ([`private-branch.txt`](private-branch.txt)) took CPU from 62 % to 1.2 % and spine growth to 0, but RSS stayed flat at **4.6 GB from +15 s**. A 6 s `sample` of the private daemon's boot put 1,549 of 3,025 busy samples in `GovernanceService::observe_deliveries` → `reconcile_deliveries` → `EventBus::tail(None, Seq(0))`. It reads the whole spine once at every boot, and again whenever its live subscription lags, to find `delivery.outcome` receipts for dispatches. For each dispatch with a receipt, `observe_delivery` also read the recipient's whole seat history. macOS malloc keeps that high-water, so the daemon stays at ~4.6 GB for its whole life even after F1–F5. The prime was not told before this fix; it is reported with the receipts.

## Regression or cliff?

**It's a cliff. None of today's merges caused it.**

- D1's full-spine `tail` has been in `EventBus::subscribe` since the initial public release (`03ab38e`, 2026-10-02).
- D2's publish-before-refusal order dates from `bcc04dd` (2026-10-05). It began burning when `pij-different-smelt`'s events source ended with its owner tombstoned, on 2026-10-26 02:24 UTC.
- D1 × D2 × chainglass's ~3.7 s attach cadence is the burn. The replay table above shows the scaling.
- #73 (control mode), #81, #83 and #84 are not implicated. The `pij-tmux-control` thread shows 12 busy samples in 10 s, daemon launches are 0.9 /s, and no D1–D4 frame belongs to code they changed.
- D3 and D4 also predate today. They cost because `open()` is expensive under Defender, not because their volume grew.

## Fixes this plan makes (each answers one line above)

| Fix | Answers | Design |
|---|---|---|
| F1 | D1 | Find the newest seq with `SELECT MAX(seq)` (a new `Spine::newest` port method). An attach then reads only the rows after its cursor. |
| F2 | D2 | Publish a job's completion event once: skip it if the spine already holds that kind and `msg_id` for the owner (`latest_matching_message`). Log a repeating background-tick failure once, not every tick. The final turn still waits for a tombstoned owner (prime ruling on review B1: resume and revive lift tombstones). |
| F3 | D3 | Keep terminal record ids in memory and stop opening them. A record never leaves a terminal state. |
| F4 | D4 | `drain_pane_tap` stats the sink first and opens it only when the length has changed. |
| F6 | D6 | Page only the dispatch recipients' `delivery.outcome` and `dispatch` rows through the spine's `(seat, kind, at)` index (`matching_since`), instead of decoding the spine or a seat's history. |
| F5 | D5 | Truncate a live tap once it has been consumed past a per-pane cap. On the observer's first pass, remove tap files whose pane is absent from `list-panes` (the decision), sparing anything written in the last 5 minutes (a brake). |

The 2.4 M `bg.lost` rows already in the spine stay there; the spine is append-only history. After F1 and F6, neither attach cost nor boot memory depends on spine size.

## After (private daemon, same recorded workload)

| | main `d459dda` | branch |
|---|---:|---:|
| CPU over 60 s | 62 % of a core | 1.9 % |
| RSS +15 s / +60 s / +120 s | 4.74 / 5.26 / 5.63 GB | 17.5 / 17.6 / 17.6 MB |
| Spine growth | 10 rows/s | 0 |
| Repeated log lines | 10/s | 1 line, once |
| One attach, 3 M rows | 6.3–7.2 s, 5.1 GB | 0.4–1.2 ms, 14 MB |

Receipts: [`private-main.txt`](private-main.txt), [`private-branch3.txt`](private-branch3.txt) (after review B1; `private-branch2.txt` was the superseded drop build), [`attach-before.txt`](attach-before.txt), [`attach-after.txt`](attach-after.txt); harness [`private-daemon.sh`](private-daemon.sh).
