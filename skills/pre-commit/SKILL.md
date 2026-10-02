---
name: pre-commit
description: |
  Run pij's canonical pre-merge / pre-report gate. Invoke before declaring
  any non-trivial task complete, before opening a PR, and after any edit
  to source, tests, manifest, or vetter pipeline. Wraps `just self-check`
  (typecheck → lint → test → smoke → pkg audit → snapshots-check) and
  encodes the contract: a task is not done until this exits zero.
---

# pre-commit

> **Encode, don't document.** This skill exists because the harness is
> the product. The gate lives in the `justfile`; this skill makes sure
> the gate actually gets run. See AGENTS.md § Self-improvement loop.

## When to invoke

- Before reporting **any** task complete to the user.
- Before opening / updating a PR or running a release flow.
- After any edit to: `.pi/extensions/**`, `harness/**`, `agents/**`,
  `.pi/packages.yaml`, `package.json`, `biome.json`, `tsconfig.json`.
- After resolving a flagged issue from a previous pre-commit failure.
- On request — when the user says "pre-commit", "pre-check", "self-check",
  "check it", or asks for a green-light before continuing.

**Do not skip** because "the change looks small." Mechanical errors
(stray imports, missing `.js` extensions, unwrapped lines) are exactly
what this gate exists to catch.

## What it runs

A single command — never compose the steps by hand:

```bash
just self-check
```

Which fans out to:

| Step               | Command                            | What it catches                                   |
|--------------------|------------------------------------|---------------------------------------------------|
| Type-check         | `npm run typecheck` (`tsc --noEmit`) | Type errors, missing `.js` import extensions      |
| Lint + format      | `npm run lint` (`biome check .`)     | Style violations, import order, unused symbols   |
| Tests              | `npm run test` (`vitest run`)        | Unit + integration regressions                    |
| Smoke              | `npm run smoke`                      | Driver-SDK end-to-end against real `pi`          |
| Manifest audit     | `PIJ_VET_SKIP_AGENT=1 just pkg audit`| Stale vetted entries, unmanifested installs       |
| Snapshot staleness | `just snapshots-check`               | Briefing.md SHA drift vs `__snapshots__/_meta.json` |

`PIJ_VET_SKIP_AGENT=1` is intentional — keeps the gate deterministic.
Live agent runs against the package-vetter pack are opt-in via
`just snapshots-refresh` or `just vet-live`, not part of pre-commit.

## Reading the result

**Exit 0 (green)** — task may be reported complete. Quote the closing
line of the output in your task summary so the user can verify (e.g.
`✓ 4 entries vetted ok` + `✓ snapshot-check: briefing.md SHA matches`).

**Non-zero exit (red)** — task is **not** done. Required response:

1. **Read the actual error** — biome/tsc/vitest output is at the bottom
   of the failure. Do not summarise from a snippet you remember.
2. **Fix the root cause.** Most failures fall into encodable patterns:
   - Biome format diff → `just format` (auto-fix). Never hand-edit
     whitespace.
   - Missing `.js` extension on relative import → fix the import.
   - Test red → fix the code or the test (not the assertion).
   - Vetter `fail` → see RUNBOOK.md § Vetting third-party extensions.
3. **Re-run `just self-check`** until green. Do not report green from
   memory.
4. **If it fails repeatedly the same way** — that is a difficulty.
   Append to `docs/difficulties.md`, and consider whether the
   `justfile` or a lint rule could prevent the next agent from hitting
   it. **Encode, don't document.**

## What this skill is NOT

- It is **not** a substitute for reading errors. Agents that say "tests
  failed, retrying" without diagnosing waste tokens.
- It is **not** the place to add new checks. New checks go in the
  `justfile`'s `self-check` recipe — that's the single source of truth.
- It is **not** a replacement for the agent harness retro. After the
  gate is green, the retro / magicWand for the session still applies.

## Contract

By invoking pre-commit, the agent commits to:

1. **Never claim a task complete with a red pre-commit.** No "tests are
   failing but unrelated" — fix or escalate.
2. **Quote evidence**, not vibes. The closing lines of `just self-check`
   are the proof the user reads.
3. **Encode new failure modes.** If pre-commit missed something a human
   reviewer caught, the gate has a gap — propose a recipe extension.
