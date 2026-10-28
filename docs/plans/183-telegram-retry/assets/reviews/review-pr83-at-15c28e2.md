# Review: PR #83 (plan 183) at 15c28e2 — seat pij-review-pr83

**Verdict: merge after fixes. 1 blocker, 1 should-fix, 4 nits.**

The core of the PR holds up. The FIFO claim, backoff, give-up, enqueue_all atomicity, timeouts, token safety and cost all check out, and every injection I ran turned red (table below). The defects are both in **staged-copy lifetime**: in two places, a copy that a live job still needs gets deleted.

## B1 (blocker): a conflicting resend deletes a staged copy an EARLIER live row still sends

`crates/daemon/src/http/mod.rs:3876`. On `Conflict`, the code releases every document whose staged path is not in **the one conflicting row's** `live_payload`. Staged names are `sha256-msgkey(msg_id#n)`, so an identical file under the same `#n` stages to the same path as a *different* live row. The filter never checks that row.

Scenario: send `[a, b]` with msg_id `m`, so live rows are `m#1` and `m#2`. The agent fixes the second attachment and resends `[a, c]` with `m`. That conflicts at index 1 (`m#2`). Path `Pa` is not in `m#2`'s payload, so it is released, but live `m#1` still sends `Pa`. The worker then fails `m#1` terminally ("staged copy … is unreadable", telegram.rs:755). The first send was answered "queued" and never reaches the phone.

Evidence: a probe test in `shim_http.rs`, removed afterwards and saved at `/Users/jordanknight/pi-hacking/review-83-probes.patch`.
```
second send: 400 Bad Request Some("… msg_id `doc-probe#2` is still queued as sidecar job 2 with different content …")
live m#1 copy exists: false
live m#2 copy exists: true
```
The PR's N1 test (`a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy`) sends only a single file, so it cannot see this.

**Same root, two more call sites:**
- **`Err` path, `http/mod.rs:3889`.** This is the coder's noted edge. `release_staged(&documents)` deletes every copy, including any that an identical live row shares. It is rarer (it needs a store error) but it is not harmless.
- **Pre-existing, `telegram_document.rs:252`.** A staging refusal on a later file releases the earlier files' copies, and those may be live rows' paths. Probe 2: send `[a, b]` with `m`, then resend `[a, missing]` with `m`; the response is a 400 and `live #1 copy exists: false`.

**Fix, at one seam:** never release a staged path that a pending or running row's payload references. Either have `enqueue_all` / `Conflict` return the live paths, or check each document (at most 10) before deleting. Then add a multi-file test (send `[a,b]`, resend `[a,c]` under the same id) as the witness.

## S1 (should-fix): the N2 sweep can delete a live job's copy; it is a pure age policy

`telegram_document.rs:163` and `telegram.rs:422`. The doc labels it honestly: "A RETENTION POLICY, not a brake". But the packet's rule is NEVER delete a copy that a pending, claimed or retrying job needs, and the sweep never consults the queue. There are two routes:

1. **The daemon was down for more than 7 days with Telegram rows pending.** The sweep runs on the **first** outbound pass, before `run_outbound`, so it deletes those copies and each job then fails "unreadable". The doc comment acknowledges this case.
2. **The safety argument's bound is wrong.** "A live job retries for about 13.5 minutes at most" (`telegram.rs:53-55` and the sweep doc) ignores two things:
   - `retry_after` is honoured up to 3600 s per attempt, so 9 retries can wait about 9 h;
   - each attempt can spend `UPLOAD_TIMEOUT` = 600 s, which adds up to 100 min.

   The worst case is about 10.7 h per job, and FIFO serialises jobs per chat (a body plus 10 files is about 5 days, pathologically). This still fits inside 7 days in practice, but the rule rests on a false number.

**Fix:** keep the age policy, and add a brake that skips any file referenced by a pending or running row. That is one SQL read every 6 h and launches nothing. With it, "never" holds by construction. Also correct the 13.5-minute claim in both doc comments and `docs/how/pij-telegram.md`.

## Nits

- **N1. `crates/daemon/src/lib.rs:585` literal `"telegram-attachments"`.** I verified it is harmless today: the producer uses `pane_signal_dir.parent()` = `state_dir` → `<state>/sidecars/telegram-attachments`, and the sweep uses `state_dir.join("sidecars").join(ATTACHMENTS_DIR)` (lib.rs:860, 994; runtime.rs:52). It is still a cross-runtime literal seam that no test pins. Use `pij_sidecars::telegram::ATTACHMENTS_DIR`.
- **N2. A panic kills one loop, permanently and silently.** The two loops are separate `tokio::spawn` tasks (`common.rs:50`) with no restart and no detection. That is better than before, when one panic killed both. The answer to "neither or both?" is "**only the one that panicked, for good**". An inbound panic would mean Jordan still receives messages but his replies go nowhere, with nothing alerting. This is pre-existing in kind; flag it for a follow-up.
- **N3. Shutdown order (`telegram.rs:364`).** `TelegramLoops::shutdown` joins outbound before it even signals inbound. Outbound may be mid-upload for up to 600 s, which delays a daemon bounce by up to 10 min. Before this PR it was unbounded, so this is an improvement. Send both stops first, then join both.
- **N4. The inbound loop has no error backoff.** If `getUpdates` fails fast (DNS or connection refused), it fails about 4 times a second, every 250 ms tick, with an `eprintln` each time. The rate is unchanged from main (pre-existing) and launches nothing, but it floods the log while Telegram is unreachable.

## Verified sound (focus list)

- **Two loops, shared state.** Bindings are append-only spine events, written by outbound (`bind_reply`, telegram.rs:954) and read by inbound via `latest_matching`. The cursor is inbound-only. The `reqwest::Client` is shared and is `Arc`-safe. There is no read-modify-write between the loops, so no lost update.
- **FIFO claim (`queue.rs:276-283`).**
  - Scoped to `sidecar:telegram:%` and bound as data. Only `send` and `document` match, and both are claimed by the same worker.
  - Different chats stay independent (the `serial_key` correlation).
  - A crash mid-attempt cannot block a key forever: the row stays `running` until the next telegram claim expires it after `claim_lease_secs` = 300 s (`core/src/config.rs:199`, queue.rs:196-215), then marks it `failed`. The worst case is a 5-min hold, and the send is lost with no telegram spine event (pre-existing).
  - Only one claimer exists, so the 600 s upload is never lease-expired mid-flight.
- **Backoff.** 2·2ⁿ capped at 300 s; at most 10 attempts; `max(backoff, retry_after)`; gives up when `retry_after` exceeds 3600. Give-up writes `telegram.outbound-gave-up` and then acks Failed. The whole-second ceiling now covers `retry` for every queue user, and zero keeps the database clock.
- **`enqueue_all`.** One transaction. A conflict rolls back. The collapse read-back is the same as `enqueue`'s. The existing test asserts `live_payload` carries no token.
- **Token safety.** Upload, `sendMessage` and `getUpdates` errors all pass through `http_error` (`without_url`), including timeouts; mutant S5 confirms a witness exists.
- **Cost.** No `Command` in `telegram*.rs`. The claim's subquery uses `jobs_serial` (from the coder's EXPLAIN receipt). The inbound pass is a 1 s long poll, about 1 call per second, the same as before.

## Injections run by me (each restored; every one turned red)

| # | mutation | killed by |
|---|---|---|
| I1 | `FIFO_KIND_PATTERN = "%"` | `other_kinds_are_not_held_behind_a_backing_off_row` |
| I2 | drop `earlier.serial_key = jobs.serial_key` | `telegram_rows_on_one_chat_keep_fifo…` |
| I3 | Conflict commits instead of rolling back | `sqlite_enqueue_all_reports_fresh_and_collapsed_rows` |
| S1 | give-up event kind changed | `a_job_that_keeps_failing_backs_off_then_gives_up` |
| S2 | attempt cap off by one | 3 retry tests |
| S3 | sweep ignores age | `the_sweep_removes_copies_older_than_the_retention_only` |
| S4 | backoff cap removed | `backoff_doubles…`, `a_job_that_keeps_failing…` |
| S5 | upload error keeps the URL | `the_token_never_leaks_on_any_media_failure_path` |

The coder's M1–M8 receipts (`evidence/mutants.md`) are consistent with this. The X2 traceback in that file is a first scripting miss; it was re-run below and killed.

## Tests run

- `cargo test -p pij-store --test queue_retry_order --test queue_enqueue_all`: 4 + 3 passed.
- `cargo test -p pij-sidecars telegram`: 53 + 2 passed.
- `cargo test -p pij-daemon --test telegram_order --test shim_http -- --skip review_probe`: 1 + 24 passed.
- `cargo test -p pij-cli a_symlinked_attachment`: 1 passed.

## Not run

- `harness checks`, clippy, fmt and the full workspace: the scope was a read-only review, and CI receipt 40659555672 is cited.
- No live fleet reading, not even a ping: there are zero launch sites to measure.
- No real Telegram, per the packet.
- I did not test HTTP/2 multiplexing (whether a stalled upload and `getUpdates` sharing one h2 connection could starve each other); I believe reqwest defaults to h1 here, but that is unverified.
