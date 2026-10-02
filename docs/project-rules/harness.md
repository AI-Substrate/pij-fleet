# pij — Engineering Harness Contract (BIO loop)

> The Boot / Interact / Observe contract any agent can use to validate
> work on pij autonomously, without reading the full RUNBOOK or AGENTS.md.

> **Scope**: this doc covers the **engineering** harness — the substrate
> layer (`npm install`, `pi`, `npm run self-check`, tmux smoke). The
> **agent** harness (minih companion-mode review) is documented separately
> at [`agent-harness.md`](agent-harness.md). Both layers are required for
> a fully-validated plan-6 build; removing either weakens the proof. See
> [`agent-harness.md` § Layering](agent-harness.md#layering--agent-harness-vs-engineering-harness)
> for the contract split.

**Maturity**: L2 — auto boot via npm; deterministic observe via
`npm run self-check`; tmux-driven interact for end-to-end smoke.

## Boot

```bash
npm install
```

Done when: `npm install` exits 0. Network required; ~20–30 s the first
time, much faster on subsequent runs (npm cache).

## Interact

Two interaction modes, depending on what you're validating.

### Interactive (TUI)

```bash
pi
```

From the pij root. Pi auto-loads any extension in `.pi/extensions/<name>/`.
Type slash commands (`/<name>`); evidence comes from the rendered TUI.
Use this when you need a human-in-the-loop or when debugging behaviour
that doesn't yet have a smoke scenario.

### Automated (smoke)

```bash
npm run smoke -- <name>      # one extension
npm run smoke                # all scenarios
```

Drives pi inside a fresh tmux session per scenario. Each step sends
keystrokes and asserts a regex against the captured pane. Exit 0 ⇒ pass.

## Observe

```bash
npm run self-check
```

Runs in fail-fast order: `typecheck → lint → test → smoke`. Step 4
(smoke) requires tmux + the `pi` binary on PATH; CI runs steps 1–3
only (smoke is local until a SDK-driven smoke is built — see
`docs/difficulties.md` D-008).

Done when: exit 0. The wall-clock for a full self-check is recorded in
`docs/velocity.md` as evidence for the compounding hypothesis (see spec
§ Clarifications session 2026-05-09b — no fixed minute thresholds are
gates; we measure and compare against the v1 baseline).

## Hard failures (stop work, file a difficulty)

- `npm install` fails → check Node ≥ 20, network, package-lock integrity.
- `npm run typecheck` fails → P6 (structural entry types — no `as` at
  the boundary) or P7 (`.js` extension on relative imports) is the most
  common cause. Numbering matches AGENTS.md / workshop 003.
- `npm run lint` fails → run `npm run format` and re-check.
- `npm test` fails → the store layer (P2/P8). Tests target `store.ts`,
  not the pi-side wiring.
- `npm run smoke -- <name>` fails → either tmux/pi env (check `which
  tmux && which pi`), or the scenario's regex doesn't match (capture the
  pane manually with `tmux capture-pane -p` to see what pi rendered).

## Soft failures (note, continue)

- pi displays "Update Available" — informational, harness still works.
- pi prints "tmux extended-keys-format" warning — cosmetic; add the
  recommended line to `~/.tmux.conf` if you care.

## Health check (single line)

```bash
npm run self-check && echo "HARNESS_HEALTHY" || echo "HARNESS_BROKEN"
```

Use this at the start of any session that intends to author or modify
extensions. If `HARNESS_BROKEN`, fix the harness before any extension
work. **The harness IS the product.**

## History

| Date | Change |
|------|--------|
| 2026-05-09 | v0.1.0 — harness shipped (Phase 1–6); BIO contract first lands here as part of Phase 5 (T028). Spec § Clarifications 2026-05-09b removed fabricated minute targets; this contract is measurement-anchored only. |
| 2026-05-11 | v0.3 — Driver SDK at `harness/driver/` becomes the typed substrate for smoke; `npm run smoke` is now an adapter. `extension-validator` agent pack (`agents/extension-validator/`) drives the SDK autonomously. Plan 004. |
| 2026-05-14 | `npm run link` (`harness/scripts/link-global.ts`) for cross-cwd use; `npm run pkg` (`harness/scripts/packages.ts`) + `.pi/packages.yaml` for third-party extension manifest with enable/disable + `pi remove` on disable. `.mcp.json` at repo root for MCP server configs (read by `pi-mcp-adapter` if installed). |
| 2026-05-15 | `session-sql` implementation tightened extension templates (`setStatus(..., undefined)` and current Driver SDK smoke shape), added a Vitest `node:sqlite` shim, and made Driver SDK `waitIdle()` tolerate extension status lines after the model footer. |
| 2026-05-15 | Plan 009 — supply-chain vetting added. `npm run pkg vet <source>` / `pkg audit` run a layered pipeline (`npm audit`, `lockfile-lint`, `github-trust`, OpenSSF Scorecard, + a `minih` agent at `agents/package-vetter/` applying the workshop-001 rule taxonomy). Each manifest entry carries a `vetted: { date, score, overrides?, agentRubric? }` block; `pkg bootstrap` refuses stale (>30d) entries without `--unsafe`. `self-check` now ends with `PIJ_VET_SKIP_AGENT=1 npm run pkg audit`. AGENTS.md gained a "Security protocol" section documenting the `requires.install` shell vector. |
| 2026-05-15 | Plan 009 FX001 close-out — closed 4 HIGH findings from a `code-review` minih agent audit of the Plan 009 landing. Typed `vetted.overrides: { rules, reason }` (rule-scoped acceptance via single `parseOverrides()` reader; closes F004 — unrelated warns no longer masked). `cmdAudit` synthesises a `vetter:"audit"` Verdict for unmanifested project-scope installs so they gate exit code (F002). `cmdAudit` write-back refreshes `vetted.date` only on RAW `verdict.level === "ok"` — override entries age out (F001 + guards F004-via-write-back). AC-05 live evidence committed at `agents/package-vetter/__snapshots__/` (7 corpus runs + 12 raw + 4 median package runs); `briefing.md`-SHA staleness alarm added to `self-check` via `npm run snapshots:check`; opt-in regression `agent.live.test.ts` gated on `PIJ_VET_LIVE=1`. Adapter rewritten for current minih CLI (`-p key=value` + `last-run`-based report). |
| 2026-05-15 | Plan 010 — `todo` extension added a deterministic `/todo` + `/sql` smoke path and exposed a reload-race pattern that is now encoded with explicit post-`/reload` wait steps in todo and ralph-loop smoke. Final observe path `npm run self-check` passed with ralph-loop, session-sql, and todo smoke. |
