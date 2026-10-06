# Validation: f9-part2-receiver-hold-false-positive-plan.md

**Date**: 2026-10-06 · **Scope**: narrow (lead-only; self-contained bug-fix plan, no named external consumers)

## Validation Contract
- **Purpose**: Fix a false-positive, permanent receiver hold in the pij native Copilot extension (F9 part 2)
- **Promise**: The plan must fully and unambiguously specify a code change that an implementer can follow to fix the bug without weakening any existing fail-closed safety test
- **Proof target**: Decision/Implementation
- **Proof required**: cross-referenced line numbers resolve against current source; task sequencing is internally consistent with the actual retry/catch control flow; AC coverage is complete
- **Upstream**: `research-dossier.md` (ORIGIN)
- **Consumers**: the implement stage (this same session); no other named consumer
- **Position**: no public contract exposed by the plan itself
- **Sources**: `.copilot/extensions/pij/store.mjs` (current, read directly), `progress.test.mjs`

## Findings (resolved in-document before this record)

| Severity | Finding | Evidence | Fix applied |
|---|---|---|---|
| HIGH | T002/Finding-03 originally said to mirror the main loop's `holdStarted`/`elapsedMs` tracking (`store.mjs:1062-1075`) for the new `receiver-gap` holdKind — but that tracking is hardcoded to `error.holdKind === "native-session"`; any other holdKind falls into the `else` arm, which explicitly resets `holdStarted` | `store.mjs:1072-1075` read directly | Plan corrected: elapsed-time tracking now specified as self-contained (`completion.gapFirstSeenAt`, read/set inside `probeReceiver`); the main loop's catch block is explicitly called out as **not to be modified** |
| HIGH | T004 assumed `createNativeReporter` could branch on `holdKind` for a `"reconnecting"` event, but the generic `"reconnecting"` emit (`store.mjs:1077-1081`) does not pass `holdKind` in its payload at all | `store.mjs:1077-1081` read directly; confirmed the event's only existing test assertion (`progress.test.mjs:740`) checks `event.kind` only, so adding a field is non-breaking; confirmed the other `"reconnecting"` emit site (`store.mjs:791`, part-1's `reconnect()`) is unrelated | Plan corrected: T004 now includes adding `holdKind` to that emit's payload as an explicit sub-step before the reporter can branch on it |

## Additional check performed (not a finding — confirms T003 is safe as scoped)

Traced every `heartbeatController`/`holdReceiving` call site (`store.mjs:696,738,807,843,867,882,906,930,1090,1092,1227`) to check for a hidden invariant assuming "heartbeat dead ⇒ receiver dead" before decoupling them (the risk the plan itself flagged):
- `stop()` (`:737-738`) and `reconnect()`'s per-attempt reset (`:806-807`) already abort/recreate both controllers **together, at their own level** — untouched by decoupling `holdReceiving` itself.
- `keepReceiverAlive()`'s own stale-lease branch (`:930`) calls `holdReceiving` then unconditionally `return`s on the next line regardless of controller state — decoupling doesn't change that it still exits.
- `keepReceiverAlive()`'s generic catch loop (`:943-956`) only checks its own (`heartbeatController`) signal to decide whether to exit — today that signal gets aborted as a side effect of *any* unrelated `holdReceiving` call; after decoupling it won't, so the heartbeat correctly keeps renewing through an unrelated hold (this is exactly AC-05, not a regression).

No hidden invariant found. T003 is safe as scoped.

## Result

✅ **VALIDATED WITH FIXES** — 2 high findings, both corrected in-document (mechanical, evidence-pinned, no new product intent invented); one additional confirmatory check performed and recorded.
- **Thesis**: advanced — the plan's design is now internally consistent with the actual control flow it modifies.
- **Consumers**: N/A (no external consumers at plan stage).
- **Open decision**: none.
