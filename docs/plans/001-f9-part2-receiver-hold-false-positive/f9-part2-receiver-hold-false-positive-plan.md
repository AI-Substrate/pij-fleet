# F9 part 2 — bounded-retry the receiver anchor-gap hold

**Mode**: Simple
**Plan Version**: 1.0.0
**Created**: 2026-10-06
**Status**: READY
**Spec source**: unified (this file)

📚 Incorporates findings from research-dossier.md

## Business Specification

### Research Context

`research-dossier.md` traced the bug to a self-confirming failure: `probeReceiver()`'s
anchor-gap check (`store.mjs:1223-1228`) permanently holds the receiver the
very first time a 128-event backward page doesn't contain the completion's
last known event id and no terminal event has been observed yet. `holdReceiving()`
then unconditionally tears down both the receiver and heartbeat controllers
(`store.mjs:867-882`), which unsubscribes the live native-event listener
(`store.mjs:990`) for good (`store.mjs:1090-1095`). The "no further native-event
lines for 8+ minutes" evidence in the handover doc is therefore a *consequence*
of the hold, not independent proof the gap was unrecoverable — a busy/long
turn plausibly just lags the queryable event-log API (both the forward poll
and the backward probe use the same API), while the live push channel and the
model itself keep working fine underneath.

Candidate (b) from the handover (search forward for the correlated message)
doesn't add independent evidence, since forward and backward reads share the
same lagging API. Candidate (a) (retry after a delay) is only safe if
*bounded* — 5 existing tests deliberately assert a permanent gap holds rather
than retries forever (`progress.test.mjs:529-561`), including one case
(`consumption-only`) where a correlated event is already visible but held
anyway because `completion.terminal` isn't. The fix must keep those cases
failing closed, just later.

### Summary

The native Copilot extension's receiver can permanently and wrongly give up
on a message mid-turn when its own cursor-based history API lags a busy
session, even though the message was actually delivered and is being acted
on. This plan makes that one hold reason retry in place (without tearing
down the receiver or heartbeat) for a bounded window before falling back to
today's hard, non-retryable hold — and, independently, stops every hold
reason from also silently killing the heartbeat, so a hold (legitimate or
not) can no longer cascade into the separate `native-receiver-stale` failure
mode.

### Goals

- A busy/long-running native turn that merely lags the queryable event log no
  longer causes a permanent, false "message lost" hold.
- A genuinely unrecoverable gap (the message really was discarded/lost) still
  ends up held, with the same fail-closed guarantee as today — only later.
- No hold reason (new or pre-existing) additionally kills the receiver's
  heartbeat/lease, so a hold never by itself manufactures a second,
  unrelated `native-receiver-stale` failure.
- Operators get an honest, distinct narration for "retrying a receiver gap"
  versus the generic "check the daemon" advice already used for connectivity
  holds.

### Non-Goals

- Changing how `completion.observe()` decides consumption/terminal proof, or
  how `canRetryDiscarded()` decides discard-safety — both stay exactly as
  reviewed today (dossier F-07, F-08).
- Adding a new, bespoke escalation constant — this reuses the existing
  `HOLD_ESCALATION_MS` (10 min) rather than inventing a second tunable.
- Any change to the daemon side (`pij-rs`) or the `/v1/inbox/*` contract.

### Target Domains

No `docs/domains/registry.md` exists for this area of the repo (the native
Copilot extension bridge, `.copilot/extensions/pij/`, is not yet modeled as a
domain the way `pij-skill`/`flow-pair` are). Per the shared-conventions
fallback ("repos without a domain registry: the spec/plan tables are the
whole context — proceed"), this plan proceeds without a formal domain table;
Gate G7 is **N/A** for that reason, not skipped by omission.

### Testing Strategy

- **Approach**: Lightweight, test-first for the new behavior — this file
  already has an extensive `node:test` + hand-rolled fixture suite
  (`progress.test.mjs`) exercising exactly this machinery; extend it rather
  than introducing a second testing style.
- **Rationale**: The existing fixture already simulates the SDK (`native.on`,
  `eventLog.tail/read`, heartbeat lease) deterministically with fake timers
  (`gate`/`tick`/`advance`) — it's the right (and only practical) tool to
  prove bounded-retry-then-hold timing without a real Copilot SDK.
- **Focus Areas**: (1) the 5 existing "inaccessible gap must hold" tests
  still hold, just after the bounded window elapses; (2) a new test proving a
  gap that resolves via a live terminal event *before* the ceiling does NOT
  hold; (3) a new test proving heartbeats keep renewing through a hold.
- **Excluded**: no attempt to test against the real Copilot native SDK — not
  available in this environment; the handover doc's "needs confirmation
  against actual SDK behavior" caveat stays open and is called out as a risk,
  not resolved here.
- **Mock Usage**: Yes — the existing `fixture()` mock in `progress.test.mjs`,
  extended, not replaced.

### Documentation Strategy

- **Location**: `docs/how/copilot-native-extension.md` (already has an
  uncommitted addendum from the deprecated prior session covering F9 part 1 —
  this plan's phase folds in a part-2 addendum alongside it).
- **Rationale**: That doc is the existing, established home for this
  extension's operational/behavioral narrative; no new doc needed.

### Complexity

- **Score**: CS-3 (medium)
- **Breakdown**: S=1, I=1, D=1, N=1, F=1, T=2 (sum 7)
- **Confidence**: 0.75
- **Assumptions**: the live `native.on()` push channel keeps delivering
  events during a busy turn even when the queryable `eventLog` API lags
  (dossier F-07) — not independently confirmed against the real SDK.
- **Dependencies**: none outside this file + its test file + the one doc.
- **Risks**: see `### Risks & Assumptions` below.
- **Phases**: 1 (Simple Mode).

### Acceptance Criteria

1. **AC-01**: Given `probeReceiver()` finds `anchor < 0` and `!completion.terminal`
   on its first occurrence for a completion, the receiver does **not** call
   `holdReceiving()` and does **not** tear down `receiverController` /
   `heartbeatController` — it raises a `retryable: true`, `holdKind: "receiver-gap"`
   `NativeError` that the existing main-loop catch (`store.mjs:1055-1075`)
   backs off and retries in place.
2. **AC-02**: If, while retrying, a live SDK event resolves the completion
   (`completion.terminal` becomes set, e.g. via `observe()` on the live
   channel or via `canRetryDiscarded`'s own recovery), the retry loop simply
   proceeds — no `receive-held` event is ever emitted for that completion.
3. **AC-03**: If the same unresolved `receiver-gap` condition persists past
   `HOLD_ESCALATION_MS` (10 minutes of elapsed retrying for that hold
   episode), the extension escalates to today's exact behavior: calls
   `holdReceiving()` with a non-retryable error, tearing down the receiver as
   it does today.
4. **AC-04**: The 5 existing "inaccessible gap" tests
   (`empty`, `expired`, `unrelated-terminal`, `subagent-spoof`,
   `consumption-only`) still end in exactly one `receive-held` event with the
   same diagnostic family as today — only reached after the fixture's fake
   clock advances past the escalation ceiling, not on the first probe.
5. **AC-05**: For **every** hold reason (pre-existing and the new one),
   `holdReceiving()` no longer unconditionally aborts `heartbeatController` —
   a hold on the receive side does not by itself also kill heartbeat
   renewal. A test proves heartbeats keep renewing (the lease keeps getting
   renewed) across a `receive-held` event unrelated to the heartbeat.
6. **AC-06**: `createNativeReporter` narrates a `receiver-gap` reconnecting
   episode distinctly from the generic daemon-connectivity "reconnecting"
   message (which incorrectly suggests checking the Pij daemon).
7. **AC-07**: `just copilot-native-test`, `just typecheck`, and `just lint`
   all pass (real, not traced/assumed) output, per the handover doc's
   explicit requirement.

### Risks & Assumptions

- **Unconfirmed SDK mechanism** (dossier risk 1): the "deferred flush during
  a busy turn" explanation for why the queryable event log lags is inferred,
  not confirmed against the real Copilot native SDK. Mitigation: the fix
  does not depend on *why* the log lags — it only bounds how long the
  extension tolerates the lag before falling back to today's exact
  behavior, so it is safe even if the precise mechanism is never confirmed.
- **Slower failure signal for genuine loss** (dossier risk 2): a message
  that really was discarded now takes up to `HOLD_ESCALATION_MS` (10 min)
  to surface as held, instead of immediately. Mitigation: 10 min is an
  existing, already-reviewed value in this same file for an analogous
  decision, and comfortably covers the real-world recovery times reported
  (single-digit minutes).
- **Hidden coupling risk for AC-05** (dossier risk/handoff note): there is no
  git history beyond the squashed initial commit explaining why
  `holdReceiving()` coupled the receiver and heartbeat aborts. The
  implementation phase re-reads the function and its call sites for any
  invariant that assumes "heartbeat dead ⇒ receiver dead" before decoupling
  them, and calls out explicitly if one is found instead of silently
  overriding it.

### Open Questions

None outstanding — the three decisions flagged in the dossier (ceiling
value, bundling (c), `holdKind` name) were resolved with the user before
this plan was written: reuse `HOLD_ESCALATION_MS`, ship together, name it
`"receiver-gap"`.

### Workshop Opportunities

None. The design is fully determined by the dossier + the three resolved
decisions; there is no remaining shape question that benefits from a
dedicated workshop pass, and no `Spike/POC`-type unknown (the approach reuses
an existing, proven pattern in the same file rather than an unproven one).

### Clarifications

#### Session 2026-10-06

- **Q**: Process to follow for F9 part 2? **A**: `/builder` plan workflow
  (user, via `ask_user`), rather than working directly from the handover doc.
- **Q**: Escalation ceiling, bundling of candidate (c), and `holdKind` name?
  **A**: User confirmed all three of the assistant's recommendations — reuse
  `HOLD_ESCALATION_MS`, ship (c) together, name the hold kind
  `"receiver-gap"`.
- **Q**: How to proceed through the rest of the pipeline? **A**: "just
  continue until it's done, trust your recommendations" (user) — this plan,
  its implementation, review, and ship proceed autonomously on the
  assistant's judgment; notify `pij-unacceptable-behaviour` over pij
  telegram when done, and deploy + restart all seats onto the fixed build as
  soon as possible after landing.

## Planning Seam
_Refinement opportunities still open — recorded as evidence; the flow surfaces and offers these, none gate:_
- Open Workshop Opportunities: none — all resolved

| Artifact | Present? | Effect on the plan |
|----------|----------|--------------------|
| research-dossier.md | y | Informs every Key Finding and the whole design direction below |
| workshops/*.md | n | — |

## Implementation Plan

### Gate Matrix

| Gate | Check | Status | Notes |
|------|-------|--------|-------|
| G1 | Clarify | PASS | No unresolved `[NEEDS CLARIFICATION]` markers; all three open decisions resolved in Clarifications above |
| G2 | Constitution | N/A | No `docs/project-rules/constitution.md` |
| G3 | Architecture | N/A | No `docs/project-rules/architecture.md` |
| G4 | ADR Compliance | N/A | No `docs/adr/` |
| G5 | Structure | PASS | All required sections present |
| G6 | Testing Alignment | PASS | Lightweight strategy; AC-04/AC-05/AC-06 each map to a concrete, measurable test obligation below |
| G7 | Domain Completeness | N/A | No domain registry for this area (see Target Domains) |

**Status**: READY

### Summary

Convert `probeReceiver()`'s anchor-gap hold from "immediate, unconditional,
non-retryable" to "retry in place for up to `HOLD_ESCALATION_MS`, then fall
back to today's exact hard hold" — reusing the existing `holdKind`/elapsed-time
escalation pattern already proven for `"native-session"` holds. Separately,
stop `holdReceiving()` from unconditionally killing the heartbeat for any
hold reason, removing a cascade into `native-receiver-stale`. Single file
(`store.mjs`) + its test file + one doc addendum; one phase.

### Domain Manifest

| File | Domain | Classification | Rationale |
|------|--------|---------------|-----------|
| `.copilot/extensions/pij/store.mjs` | (ungoverned — no registry) | internal | Core bridge logic touched: `NativeError`, `probeReceiver`, main retry catch, `holdReceiving`, `createNativeReporter` |
| `.copilot/extensions/pij/progress.test.mjs` | (ungoverned — no registry) | internal | Test suite for the above |
| `docs/how/copilot-native-extension.md` | (ungoverned — no registry) | internal | Operational narrative addendum |

### Key Findings

| # | Impact | Finding | Action |
|---|--------|---------|--------|
| 01 | Critical | `holdReceiving()` unconditionally aborts both controllers (`store.mjs:867-882`); the live event callback returns immediately once the receiver signal is aborted (`store.mjs:990`) | Decouple: only abort `receiverController` in `holdReceiving`; heartbeat keeps running independently of any hold reason |
| 02 | Critical | 5 existing tests hard-assert immediate holding for a permanent gap (`progress.test.mjs:529-561`) | Update timing (advance the fixture's fake clock/backoff past the ceiling) rather than removing or weakening the assertions |
| 03 | High | The `"native-session"` holdKind escalation pattern (`store.mjs:1062-1075`) is the only precedent for "retry for a while, then change behavior" in this file, **but its `holdStarted`/`elapsedMs` tracking is hardcoded to `error.holdKind === "native-session"`** — the `else` branch (`store.mjs:1072-1075`) explicitly resets `holdStarted` for any other holdKind, so a `"receiver-gap"` error falling through there gets plain exponential backoff only, no elapsed-time tracking | Do **not** touch or extend the main loop's catch block. Track elapsed time per-message instead: a `gapFirstSeenAt` timestamp on `NativeCompletion`, read/set inside `probeReceiver` itself. The main loop's existing generic `else` branch (retryable → backoff → loop) is reused as-is, unmodified — it already does exactly the "retry in place" behavior needed; only `probeReceiver` decides, each time it's called, whether to throw retryable or escalate to `holdReceiving` |
| 04 | High | `NativeError`'s constructor only attaches `holdKind` when it equals `"native-session"` (`store.mjs:39`) | Widen the guard to also accept `"receiver-gap"` |
| 05 | Medium | `createNativeReporter`'s generic `reconnecting` narration says "check the Pij daemon" (`store.mjs:1604-1661`), which is wrong advice for a receiver-gap retry | Add a `holdKind === "receiver-gap"` branch with accurate narration, mirroring the existing `native-session` special-case in the same function |

### Implementation

**Objective**: Make the receiver anchor-gap hold retry in place for a bounded
window before falling back to today's exact hard hold, and stop any hold
reason from also killing the heartbeat — proven by extending the existing
mocked-fixture test suite, with no change to consumption/terminal/discard
proof logic.

**Testing Approach**: Lightweight, test-first; extend the existing
`node:test` + hand-rolled fixture suite in `progress.test.mjs`.

#### Tasks

| Status | ID | Task | Domain | Path(s) | Done When | Notes |
|--------|-----|------|--------|---------|-----------|-------|
| [x] | T001 | Widen `NativeError`'s `holdKind` guard to accept `"receiver-gap"` alongside `"native-session"` | internal | `.copilot/extensions/pij/store.mjs` | Constructing `new NativeError(msg, true, msg, "receiver-gap")` yields `.holdKind === "receiver-gap"` | Per finding 04 |
| [x] | T002 | In `probeReceiver()`'s anchor-gap branch, replace the unconditional `holdReceiving(error); throw error;` with: stamp `completion.gapFirstSeenAt ??= this.now()` on first occurrence; if `this.now() - completion.gapFirstSeenAt < HOLD_ESCALATION_MS`, throw a `retryable: true, holdKind: "receiver-gap"` `NativeError` **without** calling `holdReceiving` — it propagates untouched through the existing main-loop catch's generic, already-working retryable branch (`store.mjs:1055-1075`, the `else` arm) for in-place exponential backoff; once `this.now() - completion.gapFirstSeenAt >= HOLD_ESCALATION_MS`, call `holdReceiving()` with the current non-retryable error exactly as today. **Do not modify the main loop's catch block** — its `holdStarted`/`elapsedMs` tracking is hardcoded to `holdKind === "native-session"` only (finding 03) and must stay that way; all new timing logic is self-contained in `probeReceiver`/`NativeCompletion` | internal | `.copilot/extensions/pij/store.mjs` | AC-01, AC-02, AC-03 hold under test | Per finding 03 |
| [x] | T003 | Decouple `holdReceiving()`: only abort `receiverController`; stop unconditionally aborting `heartbeatController` | internal | `.copilot/extensions/pij/store.mjs` | AC-05 holds under test; re-read `holdReceiving` + its callers first and note in the execution log if any invariant assumed heartbeat-death-with-receiver | Per finding 01; risk noted in Risks & Assumptions |
| [x] | T004 | **(a)** The generic `"reconnecting"` emit in the main loop's catch (`store.mjs:1077-1081`) currently does not pass `holdKind` in its payload at all — add `holdKind: error instanceof NativeError ? error.holdKind : null` so `"receiver-gap"` is distinguishable from an ordinary connectivity retry (verified: this event's only existing assertion, `progress.test.mjs:740`, checks `event.kind` only — no payload shape to break; the other `"reconnecting"` emit at `store.mjs:791`, part-1's `reconnect()` loop, is unrelated/unaffected). **(b)** Add a `holdKind === "receiver-gap"` narration branch to `createNativeReporter`, distinct from the generic daemon-connectivity advice | internal | `.copilot/extensions/pij/store.mjs` | AC-06 holds under test | Per finding 05; mirror the existing `native-session` special-case shape |
| [x] | T005 | Update the 5 existing "inaccessible gap" tests to advance the fixture's fake clock/backoff past `HOLD_ESCALATION_MS` before asserting the final `receive-held` | internal | `.copilot/extensions/pij/progress.test.mjs` | AC-04 holds; all 5 tests still pass, now asserting the bounded (not immediate) hold | Covers `empty, expired, unrelated-terminal, subagent-spoof, consumption-only` |
| [x] | T006 | Add a new test: a receiver-gap retry that resolves via a live terminal event *before* the ceiling never holds | internal | `.copilot/extensions/pij/progress.test.mjs` | AC-02 holds | |
| [x] | T007 | Add a new test: heartbeats keep renewing (lease keeps renewing) across an unrelated `receive-held` event | internal | `.copilot/extensions/pij/progress.test.mjs` | AC-05 holds | |
| [x] | T008 | Append an F9-part-2 addendum to `docs/how/copilot-native-extension.md` describing the bounded-retry behavior and the heartbeat decoupling, alongside the existing (uncommitted) part-1 addendum | internal | `docs/how/copilot-native-extension.md` | Doc describes both the retry ceiling and the heartbeat decoupling in the same style as the existing part-1 addendum | |
| [x] | T009 | Run `just copilot-native-test`, `just typecheck`, `just lint`; fix until all three are green | internal | (repo-wide gate) | All three commands exit 0, real output captured | AC-07; required before review/ship per the handover doc |

### Acceptance Coverage Map

| AC | Covered by | Verified in |
|----|-----------|-------------|
| AC-01 | T002 | New/updated test asserting no `holdReceiving` call + retryable error on first anchor-gap occurrence |
| AC-02 | T002, T006 | T006's new test |
| AC-03 | T002 | T005's updated tests (escalation still occurs) |
| AC-04 | T005 | The 5 existing gap tests, updated timing |
| AC-05 | T003, T007 | T007's new test |
| AC-06 | T004 | New assertion on reporter output for a `receiver-gap` `reconnecting` event |
| AC-07 | T009 | `just copilot-native-test` / `just typecheck` / `just lint` real output |

### Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Decoupling heartbeat from hold (T003) hides an invariant not visible from reading the function alone | Low | High (could reintroduce a different hold/lease bug) | T003 requires re-reading `holdReceiving` + call sites first; surface anything found in the execution log before proceeding, don't silently override |
| Bounded retry still doesn't resolve a real busy-turn case if the live push channel is ALSO affected (unconfirmed assumption) | Low | Medium (same false-hold symptom would persist, just delayed 10 min) | Explicitly flagged as an open, unconfirmed assumption (Risks & Assumptions); SlideForge's offer to run a patched build across their fleet and report the HELD count back is the real-world confirmation signal post-ship |
| Updating 5 existing tests' timing could accidentally weaken what they prove | Medium | Medium | T005 must keep asserting the exact same final outcome (one `receive-held`, same diagnostic family) — only the number of ticks before it changes |

### Execution Log / Discoveries

- **T003 re-read (per the Risks mitigation above)**: `consume()`'s outer `finally`
  block unconditionally aborts both `heartbeatController` and `receiverController`
  whenever `consume()` itself exits, by return or throw. Decoupling the abort out of
  `holdReceiving()` therefore does **not** durably prevent heartbeat death for any
  hold that causes `consume()` to exit — i.e. every *permanent/terminal* hold,
  including the final escalated `receiver-gap` hold once the retry window expires.
  The heartbeat still dies via `finally`, just slightly later.
  T003's only durable, observable effect is during the bounded retry window itself:
  there, `consume()` never exits at all (the retryable branch just backs off and
  loops), so `holdReceiving()` is never called and the heartbeat was never at risk
  through this path — T002 alone already delivers that property. Rewriting the
  `finally` block to also spare the heartbeat on terminal holds was considered and
  rejected as out of scope: it would need to solve a real orphan-loop/duplicate-request
  risk for retryable holds like `native-receiver-stale` (`keepReceiverAlive()`'s own
  loop self-terminates on `return`, but controller-abort is what cancels an instance
  that is still genuinely running), which is materially riskier than this plan's
  validated surface.
  AC-05 is tested honestly at both levels: an end-to-end test proving the real,
  production-relevant property (heartbeat renewal continues through the bounded
  retry window — the actual fix for the reported cascade), and a narrow unit test
  calling `holdReceiving()` directly, proving the isolated code change in isolation
  without overclaiming heartbeat survival through every hold type.
- **T005 scope correction**: in addition to the 5 parametrized "inaccessible gap"
  cases named in T005 (`empty, expired, unrelated-terminal, subagent-spoof,
  consumption-only`), a 6th, previously unenumerated test — "AC2: empty queue and
  fresh idle cannot authorize resend through a stale cursor" — also exercises the
  modified anchor-gap branch and needed the identical bounded-retry-then-escalate
  timing fix. Updated alongside the other 5; no plan amendment needed since this is
  strictly more coverage of the same branch, not a scope change.
- Only 3 of the 5 originally-named gap tests (`unrelated-terminal`, `subagent-spoof`,
  `consumption-only`) actually reach the modified anchor-gap branch. `empty` and
  `expired` fail an earlier, unconditionally-non-retryable page-validity check
  before reaching that branch, so their timing is unaffected by this change and was
  left as-is.
- Final gate (T009): `just copilot-native-test` → 277 passed, 3 skipped (pre-existing,
  env-gated), 0 failed. `just typecheck` → clean. `just lint` → clean (one pre-existing
  Biome formatting violation, unrelated to this plan's diff but blocking the gate, was
  fixed mechanically in `progress.test.mjs`).
- **Review stage (delegated code-review pass) found and fixed 2 real HIGH-severity
  defects** introduced by the T002/T004 implementation itself, both confirmed by direct
  reproduction before and after the fix (reverting each fix independently was proven to
  fail its own new regression test, then both were restored together):
  1. **`gapFirstSeenAt` leak across unrelated anchor-gap episodes** (`store.mjs`,
     `probeReceiver`'s `anchor === page.events.length - 1` "caught up" early return):
     this branch returned without clearing `completion.gapFirstSeenAt`, unlike the
     function's final fall-through path. A completion that resolved a real gap via
     this branch (e.g. the forward-read path independently catching `lastEventId`
     up) retained its stale timestamp; a later, entirely new/unrelated gap on the
     same completion then reused that stale basis for `elapsedMs`, and — if enough
     wall-clock time had passed — could escalate straight to the non-retryable hold
     on its very first occurrence, reintroducing the exact false-positive this plan
     set out to fix, via a different trigger. **Fix**: reset
     `completion.gapFirstSeenAt = undefined` on this early-return path too. Covered
     by new test "AC1: gapFirstSeenAt resets when the anchor gap resolves via the
     caught-up branch, not only the fall-through path".
  2. **Receiver-gap retries forced a redundant `/v1/register` round-trip every
     backoff cycle** (`store.mjs`, the main `consume()` loop's catch): the generic
     retryable branch unconditionally set `this.registered = false` for ANY
     retryable error before its `holdKind` switch, so every receiver-gap retry
     (which carries no registration staleness at all) forced a real `/v1/register`
     RPC to the daemon on its next loop iteration — repeating for up to the full
     10-minute escalation window, contradicting the surrounding comment's own claim
     that the retry is side-effect-free apart from backoff/delay. **Fix**: skip the
     `this.registered = false` reset specifically for `holdKind === "receiver-gap"`.
     Covered by new test "AC1: a receiver-gap retry does not force a redundant
     /v1/register round-trip on every backoff cycle".
  Both fixes were re-verified against the full `just copilot-native-test` /
  `just typecheck` / `just lint` gate (279 passed, 3 skipped, 0 failed; lint and
  typecheck clean) before proceeding to ship.
