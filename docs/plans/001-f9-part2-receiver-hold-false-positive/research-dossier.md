# Research Dossier: F9 part 2 — false-positive non-retryable hold in `probeReceiver`

**Generated**: 2026-10-06T04:50:00Z
**Query**: "Fix F9 part 2: false-positive non-retryable hold in probeReceiver (store.mjs), per docs/handover-f9-part2-false-positive-hold.md"
**Effort**: Deep
**Tools**: Standard
**Evidence**: 9 current sources · 0 historical sources (only 2 commits ever touch this file; no prior plan/ADR/retro covers this code)

## The Ask

A prior (now-deprecated) prime diagnosed, but did not fix or test, a bug in
the pij native Copilot extension (`.copilot/extensions/pij/store.mjs`): a
busy/long-running turn can make `probeReceiver()` wrongly conclude a message
was unrecoverably lost and permanently hold the receiver, even though the
model actually received and acted on the message. The handover doc
(`docs/handover-f9-part2-false-positive-hold.md`) offers three unvetted
candidate fixes (a/b/c) and explicitly warns: do not widen retryability here
without equally solid proof, because a wrong fix can silently drop or
double-deliver a message. This dossier traces the actual control flow to
determine which (if any) of the candidates is safe, and what a safe fix
actually looks like.

## Answer

1. The evidence in the handover doc ("no native-event lines at all" for 8+
   minutes after the hold) is **not proof the SDK went silent** — it is
   **self-inflicted**: `holdReceiving()` unconditionally aborts
   `receiverController`, and the live native-event callback's very first line
   is `if (signal.aborted) return;` (F-03). The moment the false hold fires,
   pij stops listening to its own native event subscription, which is then
   torn down for good in `consume()`'s `finally` (F-04). So "silence after
   the hold" is a consequence of the hold, not independent evidence for it.
2. Candidate (b) ("search forward for the correlated message") is not
   reliably actionable: the same busy-turn condition that defeats the
   *backward* anchor probe also defeats the *forward* incremental poll
   (`readEvents`, F-02) — both are cursor/query-based reads against the same
   queryable event-log backing store. If that store is genuinely lagging a
   busy turn, searching forward through the same API finds nothing either.
3. Candidate (a) ("re-check after a short delay") as a blanket, unbounded
   retry is unsafe as literally stated: it would also swallow the 5
   deliberately-designed "inaccessible gap must hold" tests already in
   `progress.test.mjs` (F-05), which simulate **permanent** gaps (a cursor
   that never recovers) and assert the system fails closed rather than
   retrying forever. A fix must distinguish "temporarily lagging, will
   recover" from "genuinely gone" — a bare retry-forever does not.
4. The codebase already has exactly this two-tier pattern for a different
   hold reason: `holdKind === "native-session"` retries with a growing delay
   and only escalates its *messaging* after `HOLD_ESCALATION_MS` (10 min,
   F-06) — it never stops retrying. What's missing for the receiver-gap case
   is the mirror of that, but bounded: retry (without tearing down the
   receiver/heartbeat) for up to some ceiling, and only convert to the
   current hard, non-retryable `holdReceiving()` once that ceiling is
   exceeded — which would reproduce today's existing "inaccessible gap"
   tests' outcome, just later, and would also resolve the real-world case
   (SlideForge's busy turns resolved within minutes, well under a
   `HOLD_ESCALATION_MS`-sized window).
5. Critically: if the receiver/heartbeat are **not** torn down while this
   retry window is open, the *existing* live-event + boundary/discard-recovery
   machinery (`completion.observe()` on every live event, F-07; `canRetryDiscarded`,
   F-08) keeps working unmodified and will itself resolve genuine consumption
   or genuine discard — no change to that logic is needed or safe to make.
6. Candidate (c) ("never let a hold silently kill the heartbeat") is an
   **independent, lower-risk, generally-applicable** improvement: today
   `holdReceiving()` unconditionally aborts `heartbeatController` for *every*
   hold reason (F-03), which is what turns even a correctly-conservative,
   intentional hold into the `native-receiver-stale` cascade (F9 part 1's
   symptom) on top of whatever the original hold already reported. It does
   not touch the exactly-once decision logic.

## Evidence

| ID | Finding | Evidence | Planning implication | Confidence |
|----|---------|----------|----------------------|------------|
| F-01 | `probeReceiver()`'s anchor-gap check fires unconditionally the first time it can't find `completion.lastEventId` in a 128-event backward page, as long as `completion.terminal` isn't already set — it does **not** check `completion.consumption`, and it fires on the very first occurrence (no retry budget) | `.copilot/extensions/pij/store.mjs:1209-1228` | This is the literal bug: one inaccessible-anchor reading is treated as proof of an unrecoverable gap | High |
| F-02 | The forward incremental poll (`readEvents`) only invokes `probeReceiver` after `RECEIVER_EMPTY_READ_LIMIT` (3) consecutive **empty** forward reads — i.e. the same cursor-based `eventLog.read` API that the backward probe also uses had already reported nothing new | `.copilot/extensions/pij/store.mjs:1117-1155` | Both the forward and backward checks are the same underlying query API; a fix that only adds a second forward search (candidate b) does not add independent evidence if that API is the one lagging | High |
| F-03 | `holdReceiving()` unconditionally aborts both `receiverController` **and** `heartbeatController`; the live native-event subscription's handler returns immediately once `signal` (the receiver controller's) is aborted | `.copilot/extensions/pij/store.mjs:867-882`, `.copilot/extensions/pij/store.mjs:990` (`if (signal.aborted) return;` inside the `native.on` callback) | The "no further native-event lines" observed in production logs is caused by the hold itself (unsubscribe-on-abort), not independent proof the gap is unrecoverable | High |
| F-04 | `consume()`'s `finally` block calls `this.unsubscribe?.()`, torn down once the main loop exits because `signal.aborted` is true | `.copilot/extensions/pij/store.mjs:1090-1095` | Confirms F-03's mechanism: once `holdReceiving` fires, the live event channel is fully detached for that attempt, not just paused | High |
| F-05 | 5 existing tests (`gap` in `empty, expired, unrelated-terminal, subagent-spoof, consumption-only`) assert `held(f).length === 1` for an **unrecoverable** (never-advancing) backward window, including one (`consumption-only`) where a correlated `user.message` event **is** present in the window but `completion.terminal` is not — i.e. the suite already deliberately holds on bare, uncontiguous consumption evidence | `.copilot/extensions/pij/progress.test.mjs:529-561` | A blanket "trust consumption without terminal" loosening (what looser readings of candidate b would do) directly regresses an already-intentional safety test; any fix must keep failing closed for a genuinely permanent gap | High |
| F-06 | A different hold reason (`holdKind === "native-session"`) already retries indefinitely with exponential backoff and only **escalates its retry interval and user-facing message** after `HOLD_ESCALATION_MS` (600000ms / 10 min) elapses — it never calls `holdReceiving`/tears down the receiver for this reason | `.copilot/extensions/pij/store.mjs:1062-1075`, `:8-9` (constants), `:1604-1625` (reporter narration) | Establishes the existing, reviewed pattern for "retry for a while, then change behaviour" — reusable as the shape of the fix instead of inventing new mechanics | High |
| F-07 | Every live SDK event (not just ones found via cursor reads) is fed to `completion.observe()` via the `native.on()` subscription, independent of `readEvents`/`probeReceiver`'s cursor state | `.copilot/extensions/pij/store.mjs:985-999` | As long as the receiver isn't torn down, a message's real-world consumption/terminal will still be recognized live even while the queryable event log is lagging — no change needed to `observe()` itself | High |
| F-08 | `canRetryDiscarded()` already refuses to authorize a resend when `completion.discardUncertain` is true (set by `probeReceiver` whenever `anchor < 0`), and independently re-checks queue/processing state and re-probes before concluding non-consumption | `.copilot/extensions/pij/store.mjs:1270-1324` | This is the correct, already-reviewed place genuine "was it discarded" uncertainty is resolved; it must not be bypassed or duplicated by the fix | High |
| F-09 | `NativeError`'s constructor only attaches `holdKind` when `holdKind === "native-session"` — a new holdKind value needs this guard widened | `.copilot/extensions/pij/store.mjs:32-39` | Small, explicit touch point if a new `holdKind` (e.g. for the receiver-gap case) is introduced | High |

## Risks and Unknowns

| Item | Evidence | Why it matters | Resolution / next evidence |
|------|----------|----------------|----------------------------|
| No repro against the **real** Copilot native SDK exists or is practical here — only the mocked fixture in `progress.test.mjs` is available | `.copilot/extensions/pij/progress.test.mjs:66-310` (fixture) | The "deferred flush during a busy turn" mechanism is inferred from log evidence + code tracing, not confirmed against live SDK internals (the handover doc flags this explicitly) | The fix should not depend on *why* the queryable log lags, only on *tolerating* it for a bounded window while leaving the live channel + existing discard-recovery machinery intact — this makes the fix robust even if the precise SDK mechanism is never fully confirmed |
| A bounded retry window still means a genuinely-lost message takes longer (up to the ceiling) to surface as held, versus today's immediate (sub-second) hold | F-01, F-06 | Slower failure signal for the rare *genuine* loss case | Reuse `HOLD_ESCALATION_MS` (10 min) as the ceiling — already an established, reviewed value in this file for exactly this kind of "how long do we tolerate before treating it as broken" decision, and comfortably covers the observed real-world busy-turn recovery (SlideForge's cases resolved within single-digit minutes) |
| Existing "inaccessible gap" tests (F-05) will need to advance a fake clock/backoff past the new ceiling before asserting the final hold, instead of holding on the first probe | `.copilot/extensions/pij/progress.test.mjs:529-561` | These are deliberate safety tests; they must keep passing (eventually holding), not be weakened — only their timing assertion changes | Update them to drive the fixture's retry/backoff forward (via `f.tick`/`f.advance`) until the ceiling, then assert the hold still fires — proves fail-closed is preserved, just bounded |

## Planning Handoff

- **Preserve**: the exactly-once / fail-closed contract for every genuinely
  unrecoverable gap (F-05); `canRetryDiscarded`'s existing discard-proof logic
  (F-08) untouched; the live-event → `completion.observe()` path (F-07)
  untouched; `native-session`'s existing escalation behavior (F-06) untouched.
- **Change carefully**: `probeReceiver`'s anchor-gap branch
  (`store.mjs:1223-1228`) — convert from "hold immediately and
  unconditionally" to "retry in place (no `holdReceiving`, no receiver/heartbeat
  teardown) for up to `HOLD_ESCALATION_MS`, narrating via the existing
  `reconnecting`-style event; escalate to the current hard
  `holdReceiving()` only once that ceiling is exceeded." This needs a new
  `holdKind` (e.g. `"receiver-gap"`) threaded through `NativeError` (F-09)
  and the main retry/catch block (`store.mjs:1055-1075`) alongside
  `native-session`'s existing elapsed-time tracking, plus reporter narration
  (`store.mjs:1604-1661`) distinct from the generic "check the daemon" advice
  (which is wrong for this case — it's not a daemon-connectivity issue).
- **Also do (independent, lower-risk)**: decouple `heartbeatController.abort()`
  from `holdReceiving()` (candidate c) so *any* hold reason — including ones
  that correctly stay terminal — does not also manufacture a secondary
  `native-receiver-stale` cascade. Verify first why the two were coupled in
  the first place (there's no git history beyond the squashed initial commit
  to check — the only evidence is the code's own structure) before assuming
  no hidden invariant depends on it.
- **Likely files/symbols**: `.copilot/extensions/pij/store.mjs` —
  `NativeError` ctor, `probeReceiver`, `consume`'s main catch block,
  `holdReceiving`, `createNativeReporter`; `.copilot/extensions/pij/progress.test.mjs` —
  the 5 "inaccessible gap" tests (F-05) need updated timing, plus new
  tests for: (1) a busy-turn gap that resolves via a live terminal event
  before the ceiling (must NOT hold), (2) the same gap still unresolved past
  the ceiling (must hold, same as today), (3) heartbeat continuing to renew
  through a hold if (c) ships.
- **Decisions still required**: exact ceiling value (dossier recommends
  reusing `HOLD_ESCALATION_MS` as-is rather than inventing a new constant —
  confirm with user/workshop); whether (c) ships in the same change or
  separately; new `holdKind` literal name.
