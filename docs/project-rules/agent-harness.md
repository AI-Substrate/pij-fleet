# docs/project-rules/agent-harness.md

**Status**: Active (codified by Plan 008 T030 as AC-12 gift b).
**Successor relationship**: Companion to `harness.md` (engineering harness),
not a replacement. See § Layering at the end.

---

## Purpose

Codify **minih + companion-mode** as pij's agent harness layer. While
`harness.md` covers the engineering harness (BIO substrate L2: `npm install`,
`pi`, `npm run self-check`), this doc covers the **agent overlay**: code
review, structured retros, magic-wand feedback loop, and the protocol that
keeps live review running alongside every plan-6 implementation.

The companion is a real second pair of eyes — its own SDK session in its
own process, briefed on the same plan, watching every commit and replying
asynchronously only when it finds issues.

> **A companion is the cheapest review you'll ever buy.** It's already
> running. It's already paid for. It's watching.

---

## Maturity (L0–L5 scale)

**Current**: **L2** + companion overlay.

| Layer | Component | Status |
|-------|-----------|--------|
| L2 boot | `npm install` (pi extensions, smoke deps) | ✅ healthy |
| L2 health | `npm run self-check` (typecheck + lint + test + smoke) | ✅ healthy |
| L2 interact | pi TUI; tmux smoke driver | ✅ healthy |
| L2 observe | pi session JSONL; tmux pane capture; smoke RunReport | ✅ healthy |
| **Agent overlay** | `minih run code-review-companion` + outside-CLI for briefings/findings | ✅ healthy (with D-025 workaround applied) |
| **Agent observe** | `agents/code-review-companion/runs/<runId>/{inbox,events.ndjson,output/report.json}` | ✅ healthy |
| L3 (autonomous validation loop) | not present | ❌ — companion review is sync-to-commit, not autonomous |

To reach L3 we'd need the companion to **propose** commits (not just
review them) and have a separate verifier promote/reject them. Not in
scope for v1.

---

## BIO contract — agent harness edition

The same Boot / Interact / Observe contract applies, scoped to the
agent layer.

### Boot

```bash
# Requirement: minih CLI on $PATH (verify with `minih --version`)
# Requirement: GH_TOKEN set (Copilot CLI auth)
export GH_TOKEN=$(gh auth token)

# Spawn the companion in the background
minih run code-review-companion &

# Wait ~12s for first contact
sleep 12

# Resolve the active run id
RUN_ID=$(minih status code-review-companion 2>/dev/null \
  | jq -r '.data | select(.verdict == "active") | .runId')
```

**Boot failure modes**:

- `E122 GH_TOKEN not set` → `export GH_TOKEN=$(gh auth token)` and retry.
- `verdict: 'dead'` after >30min silent → KNOWN false positive when
  companion is mid-tool-call. Check `currentlyRunningTool` and
  `selfReportedState`; both non-null = alive.
- `state does not match inside state schema` → D-025 workaround missing.
  Verify `agents/code-review-companion/state/inside-state.schema.json`
  exists with the widened enum (idle / reading / reviewing / reporting /
  blocked / stopping). Re-run `minih doctor` to confirm
  `prompt-state-vocabulary-drift` clears.

### Interact

**Briefing** (one-shot at plan-6 start):

```bash
minih outside inbox send code-review-companion --run "$RUN_ID" \
  --type briefing \
  --subject "Plan <slug>: <Phase Title> — Power On Mode start" \
  --body "Plan: <abs path>
Spec: <abs path>
Phase: <Phase N: Title>
Tasks doc: <abs path>

Protocol:
- I will ping at every per-task commit boundary as type=task with
  subject 'review-request: T### <sha>'
- Fire-and-forget; reply only if you find issues
- I'll send a final drain ping then control:stop when the phase is done

Hazards (from Key Findings):
- <hazard 1>
- <hazard 2>

Domain context:
- <domain> + <expectations from domain.md>

Please watch for: domain compliance violations, contract drift,
anti-reinvention overlaps, scope creep beyond the task table."
```

**Per-commit ping** (after every commit boundary):

```bash
SHA=$(git rev-parse --short HEAD)
minih outside inbox send code-review-companion --run "$RUN_ID" \
  --type task \
  --subject "review-request: T### $SHA" \
  --body "Diff: git show $SHA. Watch for: <specific concerns>. Reply if findings."
```

**Fire-and-forget.** Do NOT block on a reply. The companion replies
asynchronously only if it finds issues.

**Reply chain**: when you act on a finding, ack it with
`--type ack --ack-of <finding-id> --subject "Ack: F### fixed in <sha>"`
and include the fix sha in the body. The companion will verify the fix
on the next review-request that carries the changed files.

### Observe

| What | Where |
|------|-------|
| Active run id | `minih status code-review-companion` → `.data.runId` |
| Live verdict | `.data.verdict` (`active` / `between-polls` / `dead` — see Boot caveats) |
| All findings sent | `agents/code-review-companion/runs/<runId>/inbox/inside/messages.ndjson` |
| All replies you sent | `agents/code-review-companion/runs/<runId>/inbox/outside/messages.ndjson` |
| Event stream | `agents/code-review-companion/runs/<runId>/events.ndjson` (verbose) |
| Final farewell envelope | `agents/code-review-companion/runs/<runId>/output/report.json` |

**Farewell envelope shape** (per minih companion-mode protocol):

```json
{
  "summary": "<one-paragraph review summary>",
  "findings": [
    { "id": "F001", "severity": "HIGH", "file": "...", "category": "...",
      "issue": "...", "recommendation": "...", "ackOf": "<msgId>" }
  ],
  "retrospective": {
    "workedWell": "...",
    "confusing": "...",
    "magicWand": "...",
    "magicWandTarget": "coordination|review|orient|...",
    "notes": "...",
    "coordination": { "peerUpdatesSent": N, "unresolvedPeerRequests": N, ... },
    "difficulties": [{ "id": "MH-NNN", "category": "...", "description": "...", "workaround": "...", "severity": "annoying|degrading|critical" }]
  }
}
```

Read the farewell after `control: stop`. Append the retro to
`docs/retros/code-review-companion.md` and harvest difficulties (MH-NNN)
into `docs/difficulties.md` if they apply to pij.

---

## Companion lifecycle in a plan-6 session

```mermaid
sequenceDiagram
    participant Op as Operator (plan-6)
    participant MH as minih CLI
    participant CR as Companion run

    Op->>MH: minih run code-review-companion
    MH->>CR: spawn
    Op->>MH: outside inbox send --type briefing
    MH->>CR: deliver

    loop per-task / per-milestone
        Op->>Op: git commit (a fix)
        Op->>MH: outside inbox send --type task subject "review-request: ..."
        MH->>CR: deliver
        Note over CR: async review;<br/>reads diff;<br/>sends finding(s) only if issues
        CR->>MH: inside inbox messages.ndjson (findings + summary)
        Op->>Op: read findings; ack via --type ack --ack-of <id>
        Op->>Op: if HIGH/CRITICAL: fix now + commit + new review-request
    end

    Op->>MH: outside inbox send --type drain (optional)
    Op->>MH: outside inbox send --type control body "stop"
    MH->>CR: control: stop
    CR->>CR: write output/report.json (farewell envelope)
    Op->>Op: read farewell; append retros; harvest difficulties
```

### still-needed protocol

The companion will send a `still-needed` question if it's been idle for
a while after its last review (typically 5-10 minutes). **Reply within
its idle budget** with either:

- `--type note --ack-of <still-needed-id> --subject "still working: <what's next>"`
  to keep it alive
- Or send `control: stop` to wrap up gracefully

If the still-needed question times out (~10 min without reply), the
companion will self-terminate with a polite farewell. That's fine and
expected — the farewell still gets written. The only risk is missing a
late-arriving finding you would have addressed inline. Re-boot fresh if
you have more work to review.

---

## D-025 workaround (per-clone artifact)

`agents/code-review-companion/state/inside-state.schema.json` MUST exist
locally. It carries the widened state enum
(`idle / reading / reviewing / reporting / blocked / stopping`) that the
companion's prompt uses. Without it, the FIRST state_transition the
companion attempts will be rejected by minih's strict schema validator,
which wedges the run (events stop, inbox messages durable but unread,
run never reaches `wait_for_any` long-poll loop).

The workaround landed in commit `94cbf24`. Upstream fix is tracked at
[AI-Substrate/minih#30](https://github.com/AI-Substrate/minih/issues/30)
(see also #27 origin). When minih `0.2.0` ships with FX003b's manifest
fix, the workaround can be removed.

**Healthcheck at session start**: run `minih doctor` BEFORE the first
review-request; it must clear `prompt-state-vocabulary-drift`. This is
Phase 0 T004 in any pij plan that uses the companion.

---

## When NOT to use the companion

Companion-mode is the default for any plan-6 work that touches
production code paths. **Skip it for**:

- Pure docs-only commits (no logic change to verify).
- Trivial typo fixes < 5 lines.
- Releases / version bumps (where the diff IS the message).

For everything else, run it. Companion runs are cheap (typically
< $0.10 per plan) and the cost of missing a real bug at commit-time
dwarfs the runtime spend.

---

## Layering — agent harness vs engineering harness

| Concern | Engineering harness (`harness.md`) | Agent harness (this doc) |
|---------|------------------------------------|--------------------------|
| Substrate | `npm install`, pi extensions, tmux smoke | `minih run`, outside-inbox CLI |
| Boot | `npm install` | `minih run code-review-companion` |
| Health | `npm run self-check` | `minih doctor` (must clear `prompt-state-vocabulary-drift`) |
| Interact | pi TUI; tmux for smoke | `minih outside inbox send --type {briefing,task,ack,note,control}` |
| Observe | pi session JSONL; tmux pane; smoke RunReport | runs/<id>/{inbox, events.ndjson, output/report.json} |
| Maturity | L2 | L2 + companion overlay |
| Tracks | Code velocity (compounding hypothesis) | Review quality (findings per plan; magic-wand feedback) |
| Failure modes | Boot timeout, smoke flake | D-025 wedge (mitigated), idle-budget timeout (expected) |

Both layers are required for plan-6 to claim "fully validated build".
Removing either weakens the proof.

---

## History

| Plan | Change | Date |
|------|--------|------|
| 008-ralph-loop-extension | Created. AC-12 gift b. Codifies minih + companion-mode as the standard agent overlay for every plan-6 from now on. References D-025 workaround + AI-Substrate/minih#30. Layering split from `harness.md`. | 2026-05-15 |
| 007-options-for-pi-extensions-that-do-subagents / Phase 1 | Exercised companion-mode over the Minih Workbench implementation, including inline finding reconciliation and deterministic Minih fixture/read-only pull-surface evidence for future Workbench phases. | 2026-05-16 |
| 007-options-for-pi-extensions-that-do-subagents / Phase 2 | Exercised companion-mode over the native Minih Workbench list/modal/feed implementation, including inline findings F001–F006, deterministic modal smoke, and Phase 3 handoff evidence. | 2026-05-16 |
