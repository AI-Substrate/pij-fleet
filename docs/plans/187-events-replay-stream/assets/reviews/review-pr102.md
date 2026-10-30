# Review: PR #102 (plan 187, paged event replay), head `0b05852`

Reviewer seat: `pij-review-pr102`. Read-only; worktree left clean (`git status --short` empty).

**Verdict: merge after fixes.** 0 blockers, 3 should-fix (S1 is new with this PR, S2 is a test gap at the exact ordering this PR rewrote, S3 is pre-existing and can follow), 4 nits.

The core change is correct. Keyset pages by rowid (`AUTOINCREMENT`, so no reuse; SQLite has one writer, so seq order matches commit order) neither skip nor repeat across page boundaries, from cursor 0, at the head, or at exactly one page. The `CursorBeyondSpine` 409 is unchanged.

## Should-fix

### S1. A failed later page silently stalls the local leg when federation is merged (new)
`crates/daemon/src/events/mod.rs:385-394`, `crates/daemon/src/http/mod.rs:3254-3262`

On main, the only replay read happened inside `subscribe`, so a store error refused the attach (500). Now a page-2+ error makes `Subscription` return `Ready(None)`. `http::events` merges that stream with `federation.subscribe_remote(..)` whenever federation is configured and `scope != local`. `merge` finishes only when both legs finish, and the remote `StreamMap` of broadcast receivers never finishes. The result is a 200 connection that stays open and healthy-looking, sends no more local frames and never ends. That is the "silence presented as completeness" shape F5 and F7 removed.

Scratch test (`FailingSpine` fails its 2nd `tail_page`; `sub.merge(tokio_stream::pending())`; 3 live events published after the failure):
```
test failed_page_under_merge_stalls_local_leg_silently ... pij-rs: event replay after 1000 failed: t: busy
received 1000; after page failure: Pending (connection open, no frames, no end)
ok
```
- **Scope:** chainglass recycles every 5 s, so it recovers within 5 s. Consumers that don't recycle (`.omp/extensions/pij/adapters/daemon-http.ts:398`, `crates/cli/src/client.rs:469` `pij tail`) stay stalled. I did not check whether the live machine has federation peers configured.
- **Fix:** make a page error end the whole HTTP body, not just the local leg. For example, have the local leg yield a terminal `io::Error` into the body stream (or `take_until` on local end), so the client reconnects from its cursor.

### S2. Nothing tests "live receiver created before the first page"
`crates/daemon/src/events/mod.rs:189` (the invariant the `subscribe` doc and `daemon-cost.md` cite)

Mutant: drop `let receiver = self.live.subscribe()` from the top and subscribe after the first `tail_page`. It survives everything that covers it:
```
$ cargo test -p pij-daemon --test events_bus   → test result: ok. 8 passed; 0 failed
$ cargo test -p pij-daemon --lib events        → test result: ok. 28 passed; 0 failed
```
- **Failure scenario:** an attach at or near the head gets a short first page and goes straight to live. Chainglass does exactly this every 5 s in steady state, from cursor 4157701. An event committed between that page read and the late subscribe is in neither leg and is lost with no signal.
- **Why the overlap test misses it:** `TestSpine::tail_page` pauses *before* it snapshots `events`, so the event published during the pause lands in the page either way. `crates/daemon/tests/events_bus.rs:77-95`.
- **Fix:** snapshot, then pause (or add a variant that does). The gap was inherited from main's overlap test, but this PR rewrote the ordering and leans on it.

### S3. Live lag is never reported to an HTTP client (pre-existing; this PR shrinks the window)
`crates/daemon/src/events/mod.rs:418-420`, `http/mod.rs:3243-3250`

Focus 2 answered with two scratch tests, both using `EventBus` capacity 4:
- **Lag *during* a paged replay is lossless.** Later pages cover anything the ring overwrote: 2,600 events, all delivered once and in order. `dropped_count` still reads 96 for zero lost events (see N3).
  ```
  test lag_during_replay_loses_nothing ... dropped_count after a lossless replay = 96
  ok
  ```
- **Lag *after* the final short page is read but before it drains loses events silently:**
  ```
  test lag_after_final_page_loses_events_silently ... missing=[1501, 1502, 1503, 1504, 1505, 1506] dropped_count=6
  FAILED
  ```

`http::events` never reads `dropped_count` and sends no frame for local lag. The remote leg does send `PeerState{Lagged, dropped}` (`federation/fanin.rs:518`).
- **Exposure:** more than `event_buffer_capacity` (1,024) publishes while the client drains at most one page. On main the window was the whole backlog's drain, so this PR narrows it and is not a regression. The client is still not told.
- **Follow-up fix:** end the stream on a local `Lagged`. The client reconnects from its last cursor and the spine replays the gap (simplest), or emit a local lagged frame (needs a wire change, so R4).

## Nits
- **N1.** `crates/testkit/src/contract.rs` `spine_page_contract` stops its chain loop on an *empty* page, but `Subscription` stops paging on a *short* page. A `Spine` that returns short pages mid-spine (say, a byte cap) passes the contract, and the replay then jumps to live, skipping the rest of the backlog. Assert in the loop that a page is short only when the next one is empty.
- **N2.** `crates/testkit/src/contract.rs:356-359`: the new doc block sits under the already-orphaned "Every promise the [`Queue`] port makes (R4). # Panics" header. `spine_page_contract`'s rustdoc now opens with the Queue sentence. Pre-existing misplacement, repeated.
- **N3.** `dropped_count` counts ring overwrites that the replay later covered (96 above). Nothing reads it on the `subscribe()` path today: `park_notice` and `governance` use `subscribe_live`. Its doc should say so, or the counter should reset at the replay→live switch, before anyone relies on it.
- **N4.** `lifecycle.rs` `RepeatedFailureLog::observe`: the same-tick dedupe `current.get(&source) != Some(&text)` is an equivalent mutant today, because each job yields at most one failure per tick (every branch `continue`s; `notify` and `pump` are disjoint). If a job ever yields two different texts in one tick, last-wins `insert` re-logs every tick. Low risk; one comment would do.

## Verified OK
- **Wire / R4:** hello is still the first body line; only its latency changed (it now waits for one page, about 1–22 ms in the receipts). Chainglass `rs-event-stream.ts:102` skips hello by `frame.hello === true`, `rs-client.ts:300` parses it, and `daemon-http.ts:398-420` and `client.rs:469` see the same surface. Frame format and cursor contract are untouched.
- **Dropped client:** the stream is pull-only (`poll_next` builds the next `PageRead` only when `replay` is empty), so dropping it drops the read in flight. Their test passes; their red receipt shows it failing on main.
- **Per-source log (ac-0004):** the test passes. A source missing from a tick is forgotten; a tick-level (`None`) failure forgets nothing else. Their `refusal-logged-every-tick` mutant is killed.
- **Mutants I ran:** `full = !page.is_empty()` (read until an empty page) is killed by `a_replay_reads_one_page_before_its_first_event_and_the_next_only_when_drained`. Receiver-after-first-page **survived** (S2).
- **Cost:** no new loop, timer, poll or launch site. Each attach holds one ≤1,000-row page plus the shared 1,024-slot ring. `TAIL_PAGE_SQL` plan pinned to `SEARCH … (rowid>?)`.
- **Convergence numbers:** believable. Conn 1 delivered 1,885,016 frames in 5 s, about 377k rows/s; the cursor went 0 → 1,834,152 → 3,764,956 → head in 3 connections; CPU was 135 % → 1 % and RSS stayed flat at 36 MB. Caveats: the seed has no live writers, so lag was not exercised there, and `chainglass-like.mjs` does no `onEvent` work. Real chainglass's per-frame handler rate is unmeasured; the daemon side has about 6× margin over the 60k rows/s bar.
- **PR tests run green:** `store_integration page` (2), `contract_over_fakes page` (1), `pij-daemon --lib` lifecycle + `each_stranded…` + `a_multi_page…` (10), `events_bus` (8).

## Not run
- Full `harness checks` / `just rust-check` / clippy: I relied on the PR's CI receipt. Disk is 97 % full (70 GiB free), so I kept builds scoped with `-p`.
- No private daemon or seeded replay: a 4M-row seed is over the 300k cap, and the receipts already cover it.
- Chainglass: read only, never run.
- Live daemon: one `pij-rs --json ping`; I did not determine whether federation peers are configured there (that decides S1's live exposure).

Scratch tests (not in the tree): reviewer scratch `zz_review102_lag.rs` (not committed). Drop it into `crates/daemon/tests/` to reproduce all three outputs above.
