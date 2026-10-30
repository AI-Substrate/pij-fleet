# Recheck: PR #102 at `b207016` (`git diff 0b05852..HEAD`)

Reviewer seat: `pij-review-pr102`. Read-only; worktree left clean.

**Verdict: merge.** 0 blockers, 0 should-fix. One residual nit (R1) and two observations, none needing a change.

## 1. My scratch tests re-run on the new head
```
lag_during_replay_loses_nothing              ... drain: stream open/pending after Some(2600)   ok   (dropped_count=96, nothing lost)
lag_after_final_page_loses_events_silently   ... drain: stream ENDED after Some(1500)  missing=[1501..1510]
failed_page_under_merge_stalls_local_leg_silently ... Pending                                   (expected, see below)
```
- **Test 2** now ends the stream at 1,500, the last contiguous seq, and delivers nothing past the gap. The client's reconnect from cursor 1500 replays 1501–1510 from the spine. My test still "fails" only because it asserts the old no-loss-on-one-connection shape; the behaviour is the ruled one.
- **Test 3** still pends because it merges at the `Subscription` level, and the S1 fix is one layer up, in `http::event_body`. The PR's `a_failed_replay_page_ends_the_body_even_with_a_remote_leg` covers that layer.

**Mutants on b207016, all killed:**

| mutant | killed by |
|---|---|
| S2: live receiver created after the first page (survived on 0b05852) | `replay_and_live_overlap_is_joined_once_by_store_sequence` |
| S3: gap check → `if false` | `a_lag_after_the_final_page_ends_the_stream_and_skips_nothing` |
| S1: drop `.chain(once(None))` local-end marker in `event_body` | `a_failed_replay_page_ends_the_body_even_with_a_remote_leg` (`Elapsed`) |

## 2. Can the local leg end in normal operation and cause a reconnect loop? No.
The local leg ends in exactly three cases:
- **A failed page.** This needs a store error, so it is abnormal.
- **A real gap after a lag.** A replaying subscription only, and only when the ring overwrote an event newer than `last_seq`, meaning the client fell more than 1,024 events behind live after the replay.
- **The broadcast sender closing.** `EventBus` is held by `AppState`, so in practice this happens only at process exit.

Normal operation:
- **Quiet spine:** `Pending` forever; never ends.
- **Slow client:** ends only on a real gap. The reconnect replays through paging, which is backpressured. A loop needs a client that is *persistently* slower than the publish rate, and then each reconnect is the correct catch-up, not a spurious one.
- **Chainglass:** an error ending takes its `reconnecting` path, but `established` stays true, so `attempt` stays 0 and the backoff is the minimum, not a growing one.
- **OMP extension:** `onError` only records telemetry (`rust-runtime.ts:383` `session.capture("daemon_event_error")`), and it reconnects as before.

**Observation O1.** The gap test `seq != last_seq + 1` assumes the live channel carries every spine seq contiguously. Every production `append_in_transaction` caller I sampled (registry, decisions, governance, role, queue) commits inside `bus.publish_committed*`, and `AUTOINCREMENT` rollback leaves no hole. If a non-broadcast spine write is ever added, the failure is safe: a spurious end, and only right after a lag.

**Observation O2.** `pij-rs tail` (`crates/cli/src/main.rs:1711`) now exits through a refusal envelope (non-zero) when the daemon shuts down mid-stream, where it used to return `Ok(None)` → SUCCESS. The API doc names this ("`pij-rs tail` reports the error"), so it is intended. The `io::Error` text in `event_body` never reaches a client: an aborted chunked body arrives as a transport error.

## 3. The S3 split
This split is right for every internal user:
- **`park_notice` and `governance`:** compare `dropped_count`, then re-reconcile. Count-and-continue is their recovery.
- **`delivery`:** `subscribe_live` is a wake signal, so continuing is right.
- **Lag during the replay:** keeps the stream open with nothing lost (2,600/2,600 above, and the PR's `a_lag_during_a_paged_replay_loses_nothing_and_keeps_the_stream`).

**R1 (nit, residual S3).** The split is "replaying vs live-only", not "HTTP vs internal". An HTTP attach with **no** `since` (`http/mod.rs:3242`, `subscribe_live`) still counts and continues, so it skips silently after a lag. In practice this is only a consumer's first attach before it has any cursor (daemon-http.ts with empty `cursors`, chainglass with no seats); every reconnect carries `since` and gets the gap end. Building that HTTP path with `ends_on_gap: true` would close it. Follow-up, not merge-gating.

## 4. Did the fix commits break anything? No.
- `cargo test -p pij-daemon -p pij-testkit -p pij-store`: every binary `ok`; daemon lib 663 passed; 0 `FAILED` or `panicked` lines.
- `cargo clippy -p pij-daemon -p pij-testkit --all-targets -- -D warnings`: clean. `cargo fmt --check` on both crates: clean.
- The nits landed as ruled: N1 short-page contract assert, N2 Queue doc moved back to `queue_contract`, N3 `dropped_count` doc, N4 comment.

**Not run:** full `harness checks` and smoke, and any private daemon or replay (same limits as before: disk 97 % full, row cap). Scratch tests: reviewer scratch `zz_review102_lag_v2.rs` (not committed).
