# pij — Agent Rules

> **The harness is the product.** pij runs on two harnesses, layered:
> the **engineering harness** (`just` recipes, `harness/`, smoke/driver SDK,
> seed data — what humans and CI invoke) and the **agent harness** (the
> Boot/Interact/Observe loop, the skills in `skills/`, the retros
> ledger at `docs/retros/`, this file). The agent harness sits **on top
> of** the engineering one and cannot exist without it. Every extension is
> an exercise; every difficulty is a gift to encode; every agent run is a
> usability study. **If a session ends without one of the harnesses
> improving, something went wrong.** Ground on this at session start with
> `/eng-harness-flow` (or `harness boot`) before touching code.

## Governance

Follow the skill's canonical [Governance branch — the rules](skills/pij/references/prime/rituals/bootstrap.md#governance-branch--the-rules), including PRD reconciliation and store/document authority.
Resolve `<government-root>` with `harness/scripts/government-root.sh [REPO]` (REPO defaults to cwd); the linked bootstrap covers absent/unavailable worktrees.
The live register is `<government-root>/prd/base-prd.dd.json` (edit via `ddocs`, then `ddocs validate` and `ddocs build`); the TypeScript-era register is closed at `<government-root>/prd/archive/`.

## Inherited from pi-mono (do not violate without explicit user approval)

- No `any` types unless absolutely necessary.
- No inline imports — never `await import("./foo.js")`, never
  `import("pkg").Type` in type positions, no dynamic imports for types.
  Always top-level standard imports.
- Never hardcode keybindings; use a configurable matching object
  (`DEFAULT_*_KEYBINDINGS`).
- Biome check (errors and warnings) before commit: `just lint`.
- Type-check: `just typecheck` (`tsc --noEmit`).
- Tests: `just test`. Run from the package root.
- Read files in full before wide-ranging changes; do not rely solely on
  search snippets.
- Never use `git add -A` / `git add .`. Use specific file paths.
- Never bypass hooks (`--no-verify`, `--no-gpg-sign`).
- Never `git reset --hard`, `git checkout .`, `git clean -fd`,
  `git stash` without explicit user approval.

## pij-specific (Patterns P1–P10)

1. **T2 layout by default**: `.omp/extensions/<name>/{index,store,test}.ts`.
   T1 (single file) only for <80 LOC, single-concern extensions.
2. **Pi-free store**: `store.ts` imports nothing from `@earendil-works/*`.
3. **Inject side effects via constructor.** No global mutable state.
4. **Tagged-union returns** (`{ ok, ... }`) over throws.
5. **Constants live in `store.ts`** next to the data they constrain.
6. **Structural entry types** at the boundary (no cast at the call site).
7. **`.js` extension on relative imports** (NodeNext / ESM).
8. **Tests target the store**, not the wiring.
9. **Persist before mutate** (event-sourced consistency).
10. **One handler for `session_start`**, all reasons (`startup`, `reload`,
    `new`, `resume`, `fork`).

## Pij peer harness default

- Default new pij peers to **OMP** unless Jordan explicitly specifies another
  harness. Model/provider/effort instructions still override the default; always
  discover the exact selector at runtime and canary it before use.

## Is that check a POLICY or a BRAKE? (ask what REMOVING it does)

A check that reads a timestamp, a threshold, or any other input is one of two
things, and they have opposite consequences for what the change depends on:

- a **policy** — it decides the outcome, so it **inherits whatever its input is
  wrong about**;
- a **one-directional safety interlock** — a brake, which can only ever make the
  operation *more* conservative.

**You tell them apart by asking what REMOVING the check does.** If removing it
makes the operation do *more* (delete more, send more, kill more), it was a
**brake**. If removing it makes the operation do something *different*, it was a
**policy**.

**A one-directional safety interlock is not a policy.**

Worked example (pij#183, s101). The orphaned-tap sweep reads each file's mtime
for a 5-minute grace, so "does it consult a timestamp?" is honestly **yes** — and
answering "no" would have been false while answering "yes, therefore it is
retention" would have blocked it behind an unrelated ruling (pij#204). The
resolving question was directional:

| input | decides deletion | can only spare |
|---|---|---|
| pane absent from `tmux list-panes` | ✅ the whole decision | |
| file mtime younger than the grace | | ✅ veto only |

**Age was never a reason to delete, only a reason not to** — remove the mtime
check and the sweep deletes *the same set or more*. So it is a brake, the change
depended on no ageing anchor, and it shipped independently.

Say which one it is **explicitly**, in those terms, when the change is
destructive: a reviewer looking at a 205MB deletion will otherwise assume a
retention rule governs it and go hunting for the policy.

## Searching this repo (known trap — silent, and it reads as absence)

**`rg` skips hidden paths by default, and the extension source lives under
`.omp/` and `.copilot/`.** So a repo-wide ripgrep sweep is structurally blind to the code you are
almost certainly looking for, and reports it as *not present*:

```
$ rg --files --glob '**/rust-runtime.ts'            # nothing
$ rg --files --hidden --glob '**/rust-runtime.ts'   # .omp/extensions/pij/adapters/rust-runtime.ts
```

**Always pass `--hidden` when sweeping this repo.** `grep -r` is unaffected.

Why this one bites harder than an ordinary tool default: **reading a file under
`.omp/` by explicit path works fine**, because that bypasses the traversal skip. So
a session can successfully open `.omp/extensions/...` minutes before a sweep returns
nothing for the same tree, and the absence feels *corroborated* rather than
suspicious. A tool that answers correctly when pointed and blindly when swept is
the worst available shape for this error.

Stated generally, because it is not really about ripgrep: **a probe's default scope
gets reported as a property of the repo.** "No matches anywhere" means *no matches
inside whatever this tool decided to look at*. Establish the scope before believing
an absence — an empty result is the one output that carries no evidence of what it
searched. (Found 2026-08-07 by `pij-massive-meadowlark` after three independent
citation disputes, all of which resolved as neither party being wrong.)

## Workflow

> **Canonical interface: `just`.** All composite gates live in the
> `justfile`; never compose npm or cargo steps by hand. `just` with no recipe
> lists every recipe.

0. **Fresh clone / new machine: `just install`** — single-command bootstrap:
   locked npm deps, build + link `pij-rs`, install OMP if absent, link the OMP
   and Copilot extensions plus curated OMP config, link the pij skill and the
   `pij` CLI shim, then `just doctor`. Re-run any time global state drifts.
   Start or restart the daemon with `just bounce-rs`.
1. Iterate on the OMP extension: run `omp` from the repo root and `/reload`.
   Type-check in another tab (`just typecheck`).
2. Test: `just test` (vitest) and `just rust-check` (fmt, clippy, cargo test).
   TS tests target `store.ts`-style pure modules.
3. Smoke: `just smoke` before merging.
4. **Before declaring any task done — or before ship — run `harness checks`.**
   The engineering-harness gate: it runs the full deterministic **signal
   inventory** (local-path portability → typecheck → lockfile allowlist → lint →
   test → Copilot native contracts → Rust gate → smoke → commit-trailer
   attribution) as individual stages and reports a per-sensor verdict — and
   unlike `just self-check` it runs **all** sensors, so one invocation surfaces
   every failure (`--quick` skips the heavy Rust and smoke stages). **New
   back-pressure sensors get added to `.harness/extensions/checks/` and to
   `just self-check` together.** If it exits non-zero, the task is not done.
   The `/pre-commit` skill (`skills/pre-commit/SKILL.md`) encodes the full
   contract.

## Harness tooling

- **Driver SDK** at `harness/driver/` — typed `Scenario`/`Step`/`Session`
  for tmux-driven end-to-end smoke. `harness/scripts/smoke.ts` is a thin
  adapter over it; it runs every `.omp/extensions/<name>/smoke.ts`.
- **`just link`** — machine-wide policy from the **canonical main checkout only**.
  It refuses linked worktrees, links only `pij` into `~/.omp/agent/extensions/`
  plus `~/.omp/agent/{mcp.json,models.yml}` → this repo's `.omp/`, and links the
  native Copilot extension. `just unlink` removes only pij-owned links; real
  paths and foreign symlinks are never clobbered.
- **`just rs-install`** builds `pij-rs` (release) and links it next to the
  `pij` shim in `$(npm prefix -g)/bin`. **`just bounce-rs`** rebuilds and
  restarts the daemon; **`just rs-autostart`** registers a macOS LaunchAgent.
- **`just update-omp`** reinstalls OMP from the governed npm registry, then
  re-applies the guarded links and runs **`just omp-doctor`**.
- **`just doctor`** — read-only audit: CLI shim shape, `pij-rs` on PATH, skill
  link, OMP policy and daemon health. The first diagnostic when anything drifts.
- **npm supply chain** — `.npmrc` enforces a seven-day `min-release-age`
  quarantine; `just lockfile-allowlist` fails on any lockfile source outside the
  allowed registry host; CI runs `npm audit --audit-level=high` as a hard gate.
  Set `PIJ_NPM_REGISTRY` to resolve through a mirror instead of npmjs.
- **Engineering harness (`harness` CLI + `.harness/`)** — pij has adopted the
  ai-substrate engineering harness (governance doc `.harness/engineering-harness.md`,
  extensions in `.harness/extensions/`). Two verbs matter day-to-day:
  **`harness boot`** (fast readiness proof = typecheck + test) and
  **`harness checks`** (the full ship/done gate). `harness doctor` audits what
  loaded. The CLI is an ambient tool (global npm, never a repo dep); the
  `.harness/` substrate is committed.

## Voice input — phonetic interpretation

The user drives a lot of input via voice dictation. Expect occasional
homophone swaps, adjacent-word substitutions, and minor transcription
errors. When a word seems out of place, **try the phonetic neighbour
first** before asking — common patterns:

- "MPM" → `npm`
- "to do" → `todo` (the extension)
- "pee eye" / "pie" → `pi`
- "minnie h" / "mini h" → `minih`
- "yam'l" / "yarmel" → YAML
- "just file" → `justfile`
- "pre-checking" / "pre-check" → `pre-commit` (skill)
- "scale" → "skill"

If two phonetic candidates are both plausible **and** the choice changes
what code you'd write, ask according to the clarification protocol below.
Otherwise pick the one that fits the surrounding context and proceed — the
user prefers forward motion over interrogation.

## Clarification protocol

Before guessing, **ask**. When you'd otherwise type a question to the user
in plain prose, use your harness's structured question tool instead (for
example `ask_user_question` or `AskUserQuestion`), batching 1–4 questions in
one call.

**Pij orchestration exception**: a seat or peer in pij orchestration never
invokes `ask_user_question` or any modal question UI — it asks in ordinary
inline text, persists the pending decision, and blocks only dependent work;
question ownership stays with the context owner (parents relay pointers, never
proxy). This changes the transport and ownership path, not the requirement to
ask before consequential ambiguity. Full doctrine: `skills/pij/SKILL.md`
invariants 9–10 and `skills/pij/references/prime/protocol.md`.

**Pij worktree synchronization rule**: isolation removes edit-time
serialization, not convergence-time serialization — work confined to a verified
stream worktree/branch is notify-only; synchronize at convergence or any shared
mutable resource. Full doctrine: `skills/pij/SKILL.md` invariant 11 and
`skills/pij/references/prime/rituals/batons.md`.

Use it when:

- requirements are ambiguous, conflicting, or implied rather than stated
- you'd otherwise pick a non-obvious default that could surprise the user
- architectural trade-offs need a user opinion (e.g. T1 vs T2 layout,
  vitest vs node:test, sync vs eager install)
- you're about to take a destructive or hard-to-reverse action

How to use it well:

- Batch related questions in one call — don't ping-pong.
- 2–4 concrete options per question; if you'd recommend one, put it first
  with `(Recommended)` appended.
- The tool auto-adds an "Other" free-text fallback; do not add your own.
- `header` ≤ 12 chars; it's the tab label.
- Skip the tool only for trivially-answerable questions (single yes/no in
  the middle of a confirmed plan) — prefer it whenever ambiguity is real.

## Self-improvement loop

This is the core mechanism that makes the harness compound. **Not
optional** — every session contributes back.

**Magic wands** — every agent session should leave a `magicWand` (the one
thing the agent wishes were different) and its `difficulties` (structured
friction reports) in `docs/retros/<slug>.md`. **Read existing retros before
starting work on a surface** — they are feature requests from the most
honest users of this harness.

**Difficulty ledger** — every difficulty encountered → `docs/difficulties.md`
with severity. Every workaround → either an immediate fix (encode it) or a
wishlist entry (`stretch:` tag). Resolved items get curated into the
relevant skill or this file so future runs never hit them.

**Retros ledger** — `docs/retros/` is the canonical record of agent
sessions; append each session's retrospective under `docs/retros/<slug>.md`.

**Velocity log** — every phase end → row in `docs/velocity.md` with
start/end and output. Goal: each successive extension is faster than the
last (no fixed minute thresholds are gates).

**Encode, don't document** — a wiki paragraph that says "remember to do
X" is worth nothing; an automated step that does X for you is worth
everything. Prefer a recipe, generator, template, lint rule, or pre-flight
check over prose. Pick the right home: dev-loop friction → engineering
harness (`harness/`, `just` recipes); agent-side friction (skill confusion,
missing context, prompt regression) → agent harness (`skills/`, this file).

**Agents are real users.** Their `magicWand` feedback is feature requests
from your most honest user. Treat it that way.

## When something is unclear

- Run `/eng-harness-flow` (or `harness boot`) to re-ground on the
  philosophy + the self-improvement contract.
- Check `docs/retros/` for prior agent runs against the same surface
  — magic wands and difficulties from earlier sessions often pre-answer
  the question.
- The daemon's API and refusal contracts: `docs/how/pij-rs-api.md`.
- The OMP extension API is pi's (`@earendil-works/pi-coding-agent`); OMP is a
  pi fork, so its extension types come from that package.

## Forbidden without explicit user approval

- Modifying the installed OMP binary outside `just update-omp`.
- Skipping any of P1–P10 in a new extension.
- Replacing the toolchain (npm scripts → just/make/pnpm/etc.).
- Publishing to npm.
- Pushing to a public remote. **Standing exception (Jordan, 2026-10-09):** pushing
  feature branches and opening PRs on `AI-Substrate/pij-fleet` needs no per-case
  approval ("yes no need to ask for things like that"). Merging to `main` stays with
  the prime, after review and green CI. Never push `refs/notes/*` there.
