# Engineering harness

> **AGENTS START HERE → `harness instructions`** — the CLI's baked agent
> briefing (envelope contract, role split, discovery loop). Then
> `harness instructions <verb>` per verb.

## Boot command
`harness boot` — checks `just worktree-deps --check` before `just typecheck` then
`just smoke` (`--full` selects `just test`), via `.harness/extensions/boot/`.
Missing linked-worktree dependencies return an advisory with `ready: false` and the
repair command before TS tools run; Rust-only work can continue. A green boot is
not a green `just self-check`.

## Health check
`harness boot` stage exit codes (`data.stages[].ok`/`code`): dependency readiness,
typecheck and smoke (or `--full` test) all pass ⇒ `data.ready: true`.

## Interact method
Dev loop is `pi` + `/reload` from the repo root (extensions autoload); `just` recipes
drive every composite gate. End-to-end UI interaction is the `harness/driver/` tmux
SDK (`harness/scripts/smoke.ts`).

## Observe method
`harness observe "<what>" --kind <kind>` for loop friction capture. Domain evidence:
the pij extension's own per-session records under `~/.pij/<id>/` (`event-once-*.json`
receipts + `inbox/`; the descriptor's `eventsPath` names the ndjson stream when one
exists) + minih retros (`docs/retros/*.md`).

## Deterministic signal inventory
- `just worktree-deps --check` — advisory linked-worktree tooling readiness, before TS boot stages.
- `just local-path-check` — no user-specific absolute home paths in operational files.
- `just typecheck` (`tsc --noEmit`) — type surface.
- `just test` (vitest) — store/core unit tests (P8).
- `just lint` (biome, errors+warnings) — style/correctness.
- `just lockfile-allowlist` — every package-lock.json source is an allowed host.
- `just copilot-native-test` — native Copilot extension node:test contracts.
- `just rust-check` — Cargo.lock current, fmt, clippy (`-D warnings`), workspace tests.
- `just smoke` — tmux-driven end-to-end driver SDK scenarios.
- `just self-check` — the composite merge gate over all of the above.
- **`harness checks`** — the runnable signal inventory: runs every sensor above
  as individual stages with a per-sensor verdict (runs ALL, not first-fail;
  `--quick` skips rust and smoke). The single "are we done?/ship" gate; add new sensors to
  `.harness/extensions/checks/`.

## Evidence paths
- Test/typecheck/lint output → stdout (captured by `harness boot` envelope `data`).
- Driver SDK captures → tmux panes via `harness/driver/`.
- pij runtime events → `~/.pij/<id>/events.ndjson`.
- Agent retros → `docs/retros/*.md`; difficulties → `docs/difficulties.md`.

## Injection map
_Host flow **self-fires**: pij's SDD pipeline (`the-flow`) already calls
`/eng-harness-flow --hook …` at its seams (the-flow invariant: "Harness = one door").
Nothing is woven into pij source — the-flow's `harness-seams` own the fire points._

| Seam event | Fires from | What fires it |
|---|---|---|
| session-start / pre-implement (`pre-flight`) | the-flow entry + phase edges | the-flow harness-boot seam |
| post-spec (`pre-coding`) | the-flow `awaiting-1b` (post-plan) | the-flow backpressure seam |
| phase-end (`post-coding`) | the-flow `awaiting-6` (phase end) | the-flow harness-retro seam |
| plan-complete (`post-flight`) | the-flow ship | the-flow post-flight retro seam |

## Back-pressure gaps
- Spawn/close (plan 017) end-to-end correctness relies on a tmux-gated smoke that
  skips when tmux is absent — no deterministic signal in non-tmux CI yet.
- Third-party package vetting is report-and-continue (Plan 009) — not a hard gate.
- minih agent-pack behaviour is eyeballed via retros, not asserted.

## Current maturity snapshot
**L0 — seeded at inception by `harness init`; nothing proven yet.**
<!-- The single, current L0–L4 level the harness is ACTUALLY at. Updated ONLY at
     the Improve beat (never by boot, which is read-only). See maturity-assessment.md. -->
