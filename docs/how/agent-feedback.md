# Agent feedback loop

How magic-wand wishes flow from running agents back into the pij harness.

> **The harness is the product.** Every difficulty an agent reports is a
> gift to future sessions. This doc describes the path from "agent found
> friction" to "next agent doesn't" — encoded, not just documented.

## Pipeline

```
A → B → C
│   │   │
│   │   └── curator promotes wishes to harness recipes / D-NNN rows
│   └────── human reviews docs/retros/<slug>.md
└────────── agent writes magicWand + difficulties to farewell envelope
```

Three actors, three responsibilities:

1. **Agent (A)** — writes structured friction into the farewell envelope before exit.
2. **minih runtime** — auto-harvests the retrospective from the envelope to `docs/retros/<slug>.md`.
3. **Human curator (C)** — reviews retros and promotes wishes to durable harness improvements.

## The envelope shape

Every coordinated minih agent (or one-shot agent that writes to `$MINIH_OUTPUT_PATH`) emits a farewell envelope with a `retrospective` block:

```json
{
  "retrospective": {
    "magicWand": "I wish runScenario exposed tmuxVersion in RunReport so my envelope's sessionMetadata didn't need a second probe.",
    "magicWandTarget": "project",
    "difficulties": [
      {
        "category": "driver-sdk",
        "description": "DEFAULT_PROMPT_RE = /^>\\s/m doesn't match pi v0.74's TUI footer; smoke timed out at boot until I overrode promptRe.",
        "workaround": "scenario passes a custom promptRe=/\\s•\\s\\w/ in its wait step",
        "severity": "medium"
      }
    ],
    "notes": "..."
  }
}
```

Fields:

| Field | Type | Required | What it carries |
|-------|------|----------|----------------|
| `magicWand` | string ≥10 chars | ✅ | One-paragraph wish, specific. See § Writing a good wish. |
| `magicWandTarget` | enum | ✅ | `project` \| `minih` \| `pi` \| `coordination` (validator pack `extension-validator` also writes `pi`) |
| `difficulties[]` | array | optional | Per-friction entries the curator may promote to `docs/difficulties.md` |
| `notes` | string | optional | Anything that didn't fit the wish or difficulty taxonomy |

## Writing a good wish

Three rules:

1. **Specific over abstract.**
   - ❌ "Wish things were more obvious."
   - ✅ "Wish `DriverAssertionError.toReport()` included `expectedSourceForJson` so I could echo my regex back to the user without double-escaping the backslashes."

2. **One wish.** The wand grants one per run. More wishes go in `difficulties[]` or `notes`. The loop earns more wishes by running more validations — that's the point.

3. **Bias to `project`.** Unless the friction is clearly elsewhere, target the **project** (pij Driver SDK + harness). That's where the human can encode the fix. `minih` / `pi` / `coordination` are real targets but require external coordination to fix — slower loops.

## Targets

| Target | Use when | Loop velocity |
|--------|----------|---------------|
| `project` | The pij harness — Driver SDK, smoke runner, scenario shape, error messages, capture quality, prompt regex, scratch (or the next kept extension) | Same-session fix |
| `minih` | minih runtime — state schema enum, inbox semantics, farewell envelope shape | Cross-repo PR |
| `pi` | The `pi` binary itself — rendering, behavior, missing affordances | Wait for pi release |
| `coordination` | Your own agent pack's design — input-schema gaps, prompt clarity, instructions ambiguity | Same-session fix |

## Difficulty taxonomy

Categories used by `extension-validator` (kept narrow on purpose — broader taxonomies don't earn their keep at v1):

- `driver-sdk` — SDK ergonomics, error message quality, capture window, default regex
- `scenario-author` — Step union semantics, JSON-regex wire shape, scenario discoverability
- `pi-rendering` — pi changed its render surface; SDK defaults didn't match
- `tmux` — tmux version drift, missing flag, pane geometry issue
- `minih-coordination` — state transitions rejected, inbox semantics surprised the agent
- `harness-doc` — `docs/project-rules/harness.md` (or this doc) was silent on something the agent needed
- `other` — anything that doesn't fit above

Severity per entry:

- `high` — agent couldn't complete its job
- `medium` — workaround was needed
- `low` — paper-cut worth noting

## Curator path

When an agent run completes and minih has written `docs/retros/<slug>.md`, the curator's loop is:

### 1. Triage the wish

Read `retrospective.magicWand`. Decide one of:

- **Encode** — wave the wand. Land the fix in a follow-up commit. (Most `project` wishes go here.)
- **Defer** — file a TODO in the relevant workshop or future plan. (Most `minih` / `pi` wishes go here unless they have a same-session workaround.)
- **No action — already covered** — note the existing recipe / D-NNN that addresses it. The wish was useful confirmation; the loop already encoded the fix.

### 2. Triage difficulties

For each entry in `retrospective.difficulties[]`:

- **Promote to D-NNN** — add a row to `docs/difficulties.md` with the entry's fields. Status starts `open` (or `mitigated` if the workaround is durable).
- **Map to an existing D-NNN** — note in the retros file: "duplicates D-014" or similar.
- **Drop** — the difficulty isn't actionable; record the reasoning briefly.

### 3. Record the decision in the retros file

Append a `## Curator notes (<date>)` section to `docs/retros/<slug>.md` with the disposition. Future agents inherit the curated wisdom; nothing gets lost.

### 4. Encode (if encoding)

This is the harness-is-the-product step. If the wish was "I wish X were better," **make X better** in code, in the same plan-6 session if possible. The wish + the fix + the curator note land as a small commit chain.

## SLA — informal

There is no hard SLA for curation in v1. The expectation:

- Wishes from a Power-On-Mode pilot get triaged before the next pilot ships.
- High-severity difficulties get triaged before the related extension's next major change.
- Everything else gets reviewed at the start of the next plan-3-architect run for the affected area.

The validator's pilot in plan 004 (T012) is the seed of this loop. As more extensions come (#3, #4, …), the curator step becomes part of the per-plan rhythm.

## Cross-links

- **Difficulty ledger** — [`docs/difficulties.md`](../difficulties.md) is the durable home for promoted entries.
- **Retros home** — `docs/retros/<slug>.md` is where minih auto-harvests envelopes.

## Why this exists at v1

Per spec § Documentation Strategy, the magic-wand loop is novel for pij. The other doc updates (difficulties / velocity / README / harness.md) are conventional; this page is the only first-class home for the loop's curator gate, envelope shape, and target taxonomy.

If the loop earns its keep across extensions #3 and #4 (compounding signal in `docs/velocity.md`), a future workshop 003 may formalize this further. For now, this doc is the contract.
