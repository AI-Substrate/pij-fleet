# Candidate learning — implement-code — "reuse the existing rule, don't invent" (plan 027)

**Run:** 027-pij-list-active-sessions, delegation `027-p1-active-list` (Sonnet 5 coder, Opus 4.8/Copilot reviewer). Verdict APPROVE_WITH_NOTES, first pass, no FIX loop.

**Miss-type avoided:** rule-reinvention. A "filter to active sessions" task invites a
worker to hand-roll a fresh `process.kill(pid,0)` + age threshold — a parallel,
drifting copy of liveness logic the repo already owns.

**What worked (candidate delta for the implement packet template):**
When the codebase already has the canonical rule, the packet should:
1. **Name the canonical source with file:line** (`core/state.ts:33 liveness(...)`,
   `STALE_AFTER_MS`), and an existing **call-site to mirror** (`core/cli.ts:664 liveOf`).
2. State the semantic mapping explicitly ("active" = `verdict === "active"`) so the
   worker doesn't re-derive the dead/stale boundary.
3. Require the production wiring to **thread the existing probe seam**, not a new one —
   and make the reviewer's AC check "reuse, not reinvention" with the probe identity named.

Result: the coder imported `liveness`+`STALE_AFTER_MS`, mirrored `liveOf`, and wired the
pre-existing `rt.isAlive` (= `isProcessAlive`) — zero new liveness logic. Reviewer's only
note was a *pre-existing* probe duplication (bridge's own copy vs CLI's), explicitly out of scope.

**Evidence:** Dim-0 keep-all mutation flipped 4 named assertions (both exclusion paths +
end-to-end wiring); full `harness checks` green (typecheck/lint/test/smoke/pkg-audit/snapshots).

(NOT auto-promoted — manual review before active.md.)
