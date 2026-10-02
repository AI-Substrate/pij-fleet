# Computed but unconsulted signals — and the controls that confirm themselves

> **Why this file exists.** Companion to [`green-that-lies.md`](./green-that-lies.md), landed for
> the same reason: it was cited doctrine that lived only in one seat's private memory. Named by
> `pij-chief-roadrunner`, 2026-07-26, after the stall-detector specimen; extended by
> `pij-superior-mastodon` and `pij-reasonable-dove` the same week.

> Signals that exist, are computed every tick, and are wired to nothing.

**The rule: a signal COMPUTING a state is never evidence the state GOVERNED anything.**

This is the structural twin of the parent rule — *a surface that REPORTS a state is never evidence
the state governed anything* — one layer down. Same disease, different surface. (That parent rule
is not yet landed in this repo; it is stated here rather than linked so nothing dangles.)

**Why it is worse than an absent signal.** An absent signal is visibly absent. A computed one reads
as coverage: an auditor finds `readiness.ts` classifying `busy` every tick and reasonably concludes
the system knows. **The gap is invisible precisely to the person checking for it.**

## Live instances (confirmed in code 2026-07-26)

- `core/readiness.ts:76` classifies `ReadinessState "busy"` from `BUSY_RE` over the rendered pane
  footer. `core/daemon/pane-signals.ts:332-355` runs a `BusyDensityTracker` over the live
  pipe-pane tap. **`core/daemon/watchdog-manager.ts` imports neither** — only `STALE_AFTER_MS`.
  So the daemon could see a peer was mid-turn and derived `stalled` anyway. Specimen:
  `pij-recent-porpoise`, silent 21 minutes inside one `Bash` call running Playwright + vitest,
  entirely healthy.
- `core/daemon/router.ts` (~line 136) carries `busy` as a passive signal and never makes a decision
  from it — the same shape, found separately.

**How to apply.** When you find a defect that looks like *"we need a signal for X"*, grep first for
whether X is already computed. The fix is often *"consult the wire you already have"*, which is far
smaller — and it **raises** the severity, because the system had the evidence and ignored it. When
adding any classifier, name its consumer in the same commit; if it has none yet, say so at the
definition site rather than leaving a reader to assume.

**A caution that generalises.** Two signals for the "same" thing can share the defect you are
fixing. Byte-density busy would have failed the porpoise specimen for the identical reason the
timestamp did — a peer blocked on a silent subprocess emits no bytes. Only the *footer* marker
survives a quiet tool call, because the harness keeps rendering "Working" while it waits. **Pick
the signal whose failure mode differs from the one you are replacing, not merely a second one.**

## The sharper second form

> A computed signal wired to a **notification** and nothing else is not an observable state.

This is the **mirror** of the parent rule, not a repeat of it. That rule says a surface reporting a
state is no evidence the state governed anything. This one runs the other direction: **a state can
genuinely govern something and still be invisible, because governing a notification is not the same
as being recorded.**

In `loop.ts:265-270` the daemon detects a modal, acts on it, and notifies the spawner — and
`pij state` still says `working · stale · stalled`, because nothing was written to the descriptor.
Keep the pair together; neither half implies the other.

**Two silent holes to check whenever you find a notification-only signal**, both live in that same
block: the notify was gated on `descriptor.spawnedBy`, so a root/operator seat with no parent gets
**nothing at all** — the mechanism works for anyone with a parent and hides its own gap from them;
and the "already told them" latch was in-memory daemon state, so nothing durable recorded that the
seat was blocked. A supervisor not watching at that moment had no way to learn it later.

**The companion rule about fixing it**: *when correct judgement lands in a trap, harden the trap —
do not add a rule telling people to be careful near it.* A prohibition in a packet is the last
resort, and its presence is evidence something upstream is shaped wrong. The seat that wedged had
done the right thing (escalate rather than reap) and was punished for the channel; a worker who
learns to hesitate before escalating is a worse outcome than the incident.

## The self-confirming control

The sharpest single mechanism in this family. A watchdog reported `STALLED` for a seat that was
correctly idle awaiting a human ruling:

> Had I nudged, I would have interrupted a seat that was correctly idle, and the interrupt would
> have produced activity, which the watchdog would have read as the fix working. **The control
> would have manufactured its own confirmation.**

**A control whose recommended response generates the evidence that the control was right is
unfalsifiable in normal use** — every intervention validates it, and the false-positive rate is
structurally unmeasurable. Worse than a wrong alarm: an alarm that cannot be discovered to be wrong.

**pij already has half the defence and it is aimed at the wrong actor.** `daemon.ts:525-538`
computes `watchdogAttributedPaneChange` and refuses to move the activity axis for pane movement the
watchdog itself caused. The *automated* nudge is correctly excluded — but a **prime or human**
nudging reactively is not, and that is the more dangerous case, because a person draws a conclusion
from the result. Extend the exclusion to any turn causally downstream of the notice.

**Test for this shape**: ask *"what would falsify this alarm, and does the standard response to it
destroy that evidence?"* If yes, the alarm needs a decline state — see the three-way split
(`stalled` = diagnosis with corroboration / `idle, cause unknown` / `blocked, and name it`).

> A related cost, measured: proving the watchdog interval lever worked end-to-end required a seat
> willing to stay armed and be annoyed sixteen times. **Every seat that took the ping's own advice
> removed itself from the population that could produce that evidence.** A control whose advice
> destroys the evidence for evaluating it will always look fine.

## The accurately-scoped comment a reader generalises past

The purest form of "reads as coverage". `readiness.ts:54-56` explains its truncation-robust busy
markers and is **explicitly scoped to claude**; it never claims copilot coverage. Nobody wrote a
false statement.

But copilot's footer (`◎ Working`, no ellipsis) matches none of the left-anchored alternatives, so
its only busy marker is `esc interrupt` at the RIGHT of the line — the one position truncation
reaches. A reader skimming the block comes away believing narrow panes are solved fleet-wide. One
seat did exactly that, twice, and quoted it to its PM as reassurance.

**Why it is harder to catch than a false claim: there is nothing to falsify.** The only tell is
noticing the sentence names *one* harness while the deployment runs *another*.

**How to apply.** When a comment describes protection, state what it does **not** cover in the same
breath, especially where the uncovered case is the majority population. An unstated limit is
re-derived by every reader, usually wrongly, and usually in the optimistic direction.

## The reachability form

`pij list --archived` had **never worked**. Plan 071 D1's handler branch existed, the help text at
`cli.ts:282` advertised the flag, and every invocation answered `E-ARG: unknown flag --archived`
(RC=64) because `archived` was missing from the parse-time allowlist. **Documented, implemented,
and unreachable** — the whole archived tier, dead from the day it shipped.

Worse than an unconsulted signal, for the same reason: three independent surfaces (plan, code, help)
all *assert* the feature, so anyone auditing for it finds confirmation everywhere and never runs it.
**A feature's tests, its handler, and its documentation can all be green while the entry point
rejects it** — only invoking the real CLI proves reachability.

When adding to an allowlist / registry / dispatch table, grep for the *other* entries the table is
supposed to contain; a missing row is invisible from every side except the caller's.

Same family, arrived at independently by s098 (F-701, 2026-08-08): a profiler monkey-patched
`node:fs`, which under ESM binds nothing, and printed a clean confident **empty** breakdown of a
30.9-second tick. Its rule generalises: **an instrumentation run reporting zero is broken until
shown to report non-zero for a call you know happened.**

## The structurally-checkable-comment rule

**A warning comment is the weakest guard you can ship.** Hoisting one spine read out of a 179-row
loop was a **20×** win (4.2s → 190ms) producing **byte-identical** output — so inlining it back
leaves every correctness test green. A comment saying *"re-measure before simplifying"* loses to a
plausible cleanup. Pinning the **call count** (`expect(reads).toBe(1)`) catches it structurally.

**Before writing "don't undo this", ask what observable *changes* when it is undone, and assert
that.**

---

**Related doctrine.** [`green-that-lies.md`](./green-that-lies.md) — eleven ways a gate reports
green without proving the claim; this file is its sibling one layer down. Two further rules are
stated rather than linked, because their sources are not landed: **recycled identifiers** — pids and
tmux pane ids both reset at boot, so corroborating one with the other reintroduces the same
false-live bug; only boot time or process start time breaks the tie. And **verify the live surface**
— daemon, CLI, extension and skills drift independently, so check what the running `pij` actually
resolves to rather than what the tests cover.
