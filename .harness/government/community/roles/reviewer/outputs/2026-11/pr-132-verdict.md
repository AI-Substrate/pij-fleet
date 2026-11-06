# PR #132 correctness review: final F005 delta

## Final verdict

**pass-with-notes** at `bd8331e09db997e1ab68938a037fc72d7791a29e`. Supersedes the block at `2851637` and earlier `5fbcfbd` verdict, both preserved below. **F001-F005 and N001 are closed; no correctness blocker remains.**

Reviewed only the three-file `2851637..bd8331e` delta (+101/-9): admission timeout, its HTTP regression and the API clarification. The fresh detached `review-132-bd8331e` worktree is clean and pinned correctly.

### F005 closure

Prompt admission now runs inside `timeout_at(deadline, delivery.accept(...))`, using the same original hold-time deadline as the subsequent event wait. Admission expiry yields `Ended::Bound` with an explicit "still being admitted" note, allowing the held action to proceed instead of waiting for the transport's independent grace window.

Cancellation ownership was checked against unchanged delivery code: `accept` delegates to `own_admission`, which spawns the admitted work into its service-owned task set before waiting on a reply channel. Dropping the timed-out caller discards that reply, not the owned admission continuation. This does **not** claim the admitted prompt is cancelled; its continuation deliberately remains owned.

The new `an_admission_slower_than_the_bound_still_ends_at_the_bound` HTTP test injects a five-second socket admission under a 300 ms memory bound. It asserts `ended: bound`, `then: closed`, the admission-specific note, and `300 <= waited_ms < 2000`. It passes in exact-head CI (0.461 s testcase duration). The old unbounded await would not satisfy that timing assertion; no local negative-control run was repeated or claimed.

### Exact-head evidence

Run [42153292528](https://github.com/AI-Substrate/pij-fleet/actions/runs/42153292528) is successful at exact `bd8331e`: check (22), check (24), rust-lint and rust-test. GitHub reports CLEAN. Downloaded JUnit records **1,920 tests, zero failures/errors**, including all ten memory-turn HTTP cases, the new delayed-admission case and retained typed-receipt, stale-idle and index regressions.

Read-only git HEAD/status/diff and dependent source reads were used, alongside:

- `gh pr view 132 --json headRefOid,mergeStateStatus,statusCheckRollup`: exact final head, CLEAN, four successful checks.
- `gh run view 42153292528 --json headSha,status,conclusion,jobs`: exact head, completed, success.
- `gh run download 42153292528 --name junit-linux --dir <reviewer-session-files>/pr132-bd8331e-ci`: successful; exact-head JUnit retained.

No local build, test, mutation, probe or harness gate was needed or run; no build lock acquired, product edits, commits, pushes or checkout movement. CI covers this delta without a macOS-only gap.

### Remaining non-blocking qualifications

The existing ddocs CI availability limitation and independently owned deployment-cost receipts remain explicit; this correctness verdict does not claim either was newly measured. Timing expiry is not a delivery-cancellation guarantee: the already admitted prompt retains its existing continuation ownership.

The delta adds no periodic poll or process launch. It adds a deadline wrapper to existing work and does not create another admission or memory-turn task.

**magicWand fulfilled:** the CI suite now distinguishes a bound on the whole memory turn from a timeout tested only after prompt admission has already succeeded.

---

# Historical repair verdict at 2851637

## Current verdict

**block** at `2851637fd10be349d30329e0f49704eb12519f69`. Supersedes the original `5fbcfbd` verdict, preserved below.

Reviewed the complete fourteen-file `5fbcfbd..2851637` delta (+901/-341), the memory-index addendum and relevant admission/transport contracts. **F001-F004 and N001 are closed.** One P2 remains: the hold deadline is not applied while prompt admission is pending.

The fresh detached `review-132-2851637` worktree is clean and pinned correctly. Exact-head CI run [42150037898](https://github.com/AI-Substrate/pij-fleet/actions/runs/42150037898) passes all four jobs. Downloaded JUnit records **1,919 tests, zero failures/errors**, including nine memory-turn HTTP cases. No local builds, tests, mutations, probes or harness gates were needed or run; no build lock acquired.

## Repair dispositions

| Item | Disposition at this head |
|---|---|
| F001: action after death/revive | Closed for the reported held-turn case: captured proc/pane/tombstone brake; close authority is rechecked and the close CAS uses that same reread. Self and parent replacement cases pass in CI |
| F002: held receipt/client mismatch | Closed: held compact is a valid `Receipt`; CLI restores `held` and `memory_turn` from the validated raw response, with a typed round-trip regression |
| F003: pre-delivery idle | Closed: watcher always starts unarmed and only its matching delivery event arms it; stale-idle regression passes |
| F004/addendum: INDEX ownership/escaping | Closed: explicit generated block replaces shape-based deletion; linked titles/hooks/tags are escaped; both addendum cases and repeated regeneration pass |
| N001: bound_ms | Closed: present on held answers and completed reports/events |
| Inline client timeouts | Holder compacts and closes now return held immediately and execute in a daemon task; non-holders retain the direct path |
| Repeated pending requests | One service-owned entry per seat; a repeat returns named HTTP 409, without a second task/prompt. The HTTP regression passes |
| Bounded hold from admission | **F005 remains:** the deadline is created at hold time but not enforced during prompt admission |

These are correctness dispositions, not a substitute for the independent security/performance verdicts. The incarnation check is explicitly a **one-directional safety interlock, a brake**, not an ageing policy.

## F005, P2: prompt admission can outlive the configured bound

Locations: `crates/daemon/src/http/memory_turn.rs:462-473,486-498`; hold deadline creation at `179-184`.

The background task computes its deadline at hold time, but `run` then awaits `services.delivery.accept(...)` without applying that deadline. Only after admission returns does it enter `timeout_at(deadline, events.next())`. An expired deadline cannot release the held action while that first await is still pending.

This is a supported production path, not just a hypothetical blocking fake. Set `PIJ_RS_MEMORY_TURN_SECS=1` and send the memory prompt to a Claude UDS recipient whose approval is held. `crates/transport/src/uds.rs:26-31` gives that admission a five-second `HOLD_GRACE`; `539-615` awaits the transport status before returning its receipt. During that await, the one-second memory deadline can pass without starting the held action. When admission eventually returns, the report can have `bound_ms: 1000` but several seconds of `waited_ms`. The per-seat pending entry also remains occupied during that excess admission wait.

The same structural gap applies to any admission delayed beyond the bound. Moving deadline construction earlier accounts for elapsed time after admission returns, but it does not bound the admission itself. This contradicts the documented hold-time bound.

**Required repair:** enforce the original deadline across prompt admission as well as event waiting, while preserving delivery's owned/cancellation-safe side effects. Keep one deadline; do not restart it after admission. Add a CI regression that holds prompt admission past a short configured bound and verifies that the held operation can progress at that bound, with correct reporting and no duplicate or unowned late side effect.

Proof grade: direct source/transport trace, **not** a claimed local reproduction. Existing bound tests use prompt admission that completes promptly; their successful event-wait timeout case does not cover this stage.

## CI evidence and unchanged limits

Run 42150037898 is successful for exact `2851637`: check (22), check (24), rust-lint and rust-test. JUnit confirms the typed held receipt, stale-idle, incarnation, reparenting, repeat refusal, non-holder bypass, both index-addendum inputs and concurrent note writer tests pass.

Read-only git HEAD/status/diff and source reads were used, alongside:

- `gh pr view 132 --json headRefOid,mergeStateStatus,statusCheckRollup`: exact head, CLEAN, four successful checks.
- `gh run view 42150037898 --json headSha,status,conclusion,jobs`: exact head, completed, success.
- `gh run download 42150037898 --name junit-linux --dir <reviewer-session-files>/pr132-2851637-ci`: successful; 1,919-test artifact retained.

The ddocs CI availability limitation from the original review remains; no local validator or full-duration admission check was claimed. The private-run transcript is now explicitly labeled as preceding the held/background design. Its old inline close is not presented as a fresh runtime proof of this head. The API, cost wording and close-dependent test helpers were reviewed.

Cost shape is one indexed holding lookup per action, one entry/task/subscription/prompt per admitted holder turn, one completion event and one one-shot deadline. A refused repeat does the existing lookup and in-process pending-set check, without a second task or delivery. No periodic poll or new launch site is added. Independent live-cost receipts remain performance-owned.

**Handover:** F001-F004/N001 are closed; keep blocked only for F005 and return the repaired exact head. No reviewer local build window is required.

**magicWand:** test the deadline against a deliberately delayed prompt admission, not only against the event stream after admission has already succeeded.

---

# Historical verdict at 5fbcfbd

## A. Verdict

**block** at `5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22`.

Three source-proven P2s remain: a delayed action can affect a revived incarnation, native self-compact returns a payload its CLI rejects, and a pre-delivery idle can incorrectly complete the memory turn. These findings are source analysis, not claimed local reproductions. All local builds, tests, mutations, probes and harness gates were deferred under the fleet backoff.

## B. Basis

Read the changed production modules, control and close call sites, native/OMP client paths, relevant delivery/event/registry contracts, new tests, plan 201 AC1-AC5, evidence and document-state changes. The new detached review worktree and GitHub both name the exact requested head. No checkout movement, product edits, commits, pushes or production-runtime interaction.

Exact-head CI run [42142573572](https://github.com/AI-Substrate/pij-fleet/actions/runs/42142573572) is green: check (22), check (24), rust-lint and rust-test. GitHub reports CLEAN. Downloaded CI JUnit records 1,909 tests, zero failures/errors, including all five `memory_turn_http` cases.

## C. Requested checks

| Check | Disposition |
|---|---|
| One pij/memory ddoc per note through writer | Implemented; note, rendering and INDEX share the commit |
| INDEX retains existing conventional hand-written rows; no deletion verb | Covered by source and passing CI unit/writer/HTTP cases; notes are not capped or pruned |
| One named five-minute bound with environment override | Implemented as `BOUND`, `BOUND_ENV` and service configuration |
| Receipt carries `bound_ms` | Not implemented, N001 |
| Clients do not time out before inline wait | Rust CLI installs no request timeout; OMP control send supplies no explicit short abort signal. No full-duration runtime proof claimed |
| Self action remains safe/idempotent across death/revive | Fails F001 |
| One delivery and live subscription, no polling | Ordinary `run` path does one `accept`, subscribes live before it, and waits on events with a deadline; ordering fails F003 |
| Non-holder compact/close direct | Implemented; passing CI case |
| Shared `admit_control` seam | Both immediate and memory-delayed controls call it; suitable convergence point for #130 |
| s07 complete, req-09/c-memory-hook shipped, Delivers line | Present in source/PR body |

## D. Findings

| ID | Priority | Location | Finding |
|---|---|---|---|
| F001 | P2, blocking | `crates/daemon/src/http/lifecycle.rs:363-374,411-439`; `http/mod.rs:2066-2068` | Pending actions retain the seat ID but not the authorized incarnation |
| F002 | P2, blocking | `crates/daemon/src/http/memory_turn.rs:301-315`; `crates/cli/src/client.rs:230-249` | Held compact payload is incompatible with the shipped typed CLI |
| F003 | P2, blocking | `crates/daemon/src/http/memory_turn.rs:231-249` | Delivered receipt pre-arms the watcher before buffered events are processed |
| N001 | P3, approval gap | `crates/daemon/src/http/memory_turn.rs:81-91,301-315` | Required `bound_ms` is absent from both held and completed receipts |

## E. Details and repair contracts

### F001: old action can close or compact a revived seat

The self-close background task waits, then calls `close_seat` with `expected: None`. That helper reads the current registry descriptor and passes that newly read descriptor to `tombstone_if_unchanged`. The inline parent-close path does the same after its wait. The original process/spawn identity and ownership check are not compared again.

Concrete source interleaving: a holder requests self-close; while the memory turn is pending, it dies and is revived under the same seat ID; the old memory turn reaches its bound; the old task reads the revived descriptor and successfully CAS-tombstones it, then ends its current holdings. The CAS protects only changes after that last read, not the incarnation the operation was authorized to close. `crates/store/src/registry.rs:51-73` confirms it compares exactly the descriptor supplied by the caller.

The delayed compact task likewise retains only `Msg.to` and calls `admit_control` after the wait; delivery resolves that ID against the registry again. It can therefore compact the new incarnation with the old request.

**Required repair:** capture a stable original incarnation and pending-operation identity, and veto stale actions at execution while allowing ordinary activity updates. Do not solve this by simply passing the entire pre-wait descriptor to a CAS that will reject legitimate activity changes. Cover self-close, parent-close and self-compact across death/revive, plus replay of the held operation.

This is a **one-directional safety interlock, a brake**, not an ageing policy: removing the incarnation veto permits more destructive closes. The old action must not acquire authority over a replacement merely because the stable seat ID was reused.

### F002: native self-compact reports failure after scheduling the action

`held` returns only `seat`, `held` and `memory_turn`. It has no `msg_id`, `outcome` or `at`. However, native `pij compact-self` calls `DaemonClient::send_control` (`crates/cli/src/main.rs:1452-1473`), whose result is still `Envelope<Receipt>`. Those three fields are required by `Receipt` (`crates/core/src/model.rs:915-921`), and the decoder explicitly converts a payload mismatch into an adapter refusal (`crates/cli/src/client.rs:732-742`).

Thus a role holder's native self-compact schedules background work, but the client reports `daemon returned the wrong payload shape` for the missing receipt fields. `pij send <self> --command compact` has the same typed call path. The current HTTP tests deserialize `Value`, so their passing held-response assertions do not cover the shipped client.

The same unchanged type also discards the inline compact's extra `memory_turn` report: `Command::Send` clears `raw_json` and reserializes the typed receipt (`main.rs:1369-1377`). A bound expiry can therefore lose its required explanation at the CLI boundary.

**Required repair:** define and carry the real control-response contract through daemon and client, including held status, request correlation and the completed memory report. Keep a client-level self-compact test and an inline bound-report round trip.

### F003: an earlier idle can satisfy the new memory turn

`run` subscribes before sending. While it awaits prompt admission, the target can publish the idle ending its previous turn. That idle is buffered in the subscription. If the prompt's send returns `DeliveryOutcome::Delivered`, `Watch` is then initialized with `delivered = true` before processing that buffer.

Consequently this valid ordering ends the wait on the wrong turn:

```text
subscribe
old turn publishes idle A
prompt is delivered; matching delivery.outcome B is published
accept returns Delivered
Watch starts delivered=true
buffered idle A immediately returns TurnEnd
```

A direct delivered receipt is a supported production path, not merely a test construct: `delivery/mod.rs:1146-1190` uses the socket transport, and `1348-1375` constructs the receipt and publishes the outcome. `core/delivery.rs:320-333` allows that socket path independently of composer idleness.

The compact/close may proceed without waiting for the prompt's own turn. Existing tests cover an idle before delivery only with `Watch::new(..., false)`; the delivered-receipt case does not model an older idle already buffered.

**Required repair:** establish the prompt-delivery boundary in event sequence order, and ignore earlier idles. A successful send receipt must not retroactively label buffered pre-delivery events as post-delivery. Add a controlled admission test that publishes an old idle before returning Delivered and does not permit the action until a later idle.

### N001: missing bound receipt field

The completed report exposes `waited_ms` but no bound; the held receipt exposes only `bound_secs`. Neither carries the approved `bound_ms`. Add it consistently to held/completed reports and `memory.turn` events. This is not the independent reason for blocking, but approval condition (a) remains unmet.

## F. Evidence and limits

CI JUnit confirms the new memory storage/index tests, concurrent writer case, note HTTP case, four watcher unit cases and five memory-turn HTTP cases pass. The three boundary scenarios above are not covered by those tests. No result from another head is used.

The ddocs sensor in check (22) explicitly reports **UNAVAILABLE: nothing was validated**. The real-ddocs test also has a skip-on-unavailable branch despite appearing as a passing testcase. The coder's committed evidence claims separate local validation; this review did not rerun it under backoff. The source flips and generated document deltas were inspected, not independently rebuilt.

The known load flakes are not findings. No local harness gate, mutation, test suite, script or live-fleet measurement ran for this review.

Cost surface: one indexed holdings lookup per action; for a holder, one prompt delivery, one live subscription, one `memory.turn` event and one one-shot deadline, plus chosen note writes. No periodic poll or per-seat-per-tick launch is added. Performance owns the deployment measurement and its independent verdict.

## G. Commands used

Read-only git status, pinned HEAD and base-to-head diffs; source/file reads; `pij report now` and review messages.

- `gh pr view 132 --json headRefOid,mergeStateStatus,statusCheckRollup`: exact head, CLEAN, four successful checks.
- `gh run view 42142573572 --json headSha,status,conclusion,jobs`: exact head, completed, success.
- `gh run view 42142573572 --job 127646225839 --log`: ddocs UNAVAILABLE warning confirmed.
- `gh run download 42142573572 --name junit-linux --dir <reviewer-session-files>/pr132-ci`: successful; `junit.xml` confirms 1,909 tests with no failures/errors and five passing memory-turn HTTP cases.

All local validation commands were intentionally **not run**, per the explicit CI-only backoff. No build lock was acquired.

## H. Handover and harness feedback

Keep blocked for F001-F003 and return a repaired exact head. Preserve the client and event-boundary cases in CI rather than requesting a local reviewer build window.

**magicWand:** a small memory-turn lifecycle matrix that crosses actor (self/parent), reply consumer (typed CLI/raw HTTP), delivery result (queued/direct), buffered old idle, and incarnation replacement. The green happy-path HTTP tests do not establish those composition contracts.

**difficulties:** CI's successful ddocs stage means unavailable, not validated; new self-held JSON bypasses the typed-client tests; source review must carry the missing incarnation and event-order invariants until the repair encodes them. These are captured here and in the reviewer memory without running local probes.
