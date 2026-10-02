# How: build pij

Everything you need to take a fresh clone of pij to a green build. The `justfile`
is the single source of truth for every command here — this article narrates it,
it never invents commands. If a step below ever disagrees with the
[`justfile`](../../justfile), the `justfile` wins.

## Prerequisites

| Need | Why | Check |
|------|-----|-------|
| **macOS or Linux + tmux** | pij spawns and observes seats through tmux; `just smoke` drives tmux end-to-end | `tmux -V` |
| **Node `>=24`, npm `>=11.10`** | [`package.json`](../../package.json) `engines`; npm 11.10 is the first release that enforces `min-release-age` | `node -v`, `npm -v` |
| **Rust** | builds `pij-rs`; the toolchain is pinned in [`rust-toolchain.toml`](../../rust-toolchain.toml) | `cargo -V` |
| **just** | the canonical command surface | `just --version` |
| **The ambient `harness` CLI** (optional) | `harness boot` / `harness checks` / `harness doctor` (global npm tool, **not** a repo dep) | `harness --version` |

The `harness` CLI is an *ambient global tool*, while the `.harness/` substrate
in this repo is committed configuration it reads — see
[`AGENTS.md`](../../AGENTS.md) (§ Harness tooling) and
[`.harness/engineering-harness.md`](../../.harness/engineering-harness.md).

## One-command bootstrap

The bootstrap is idempotent — safe to re-run any time global state drifts.

```bash
git clone https://github.com/AI-Substrate/pij-fleet.git && cd pij-fleet
just install
just bounce-rs        # start the daemon
```

`just install` runs five ordered stages:

1. **`npm ci --min-release-age=null`** — replay the committed root lockfile with
   the npm/cli #9005 compatibility exception described below. The bootstrap
   explicitly strips inherited npm policy overrides and still enforces the
   governed registry plus online revalidation.
2. **`just rs-install`** — `cargo build --release --locked -p pij-cli`, then link
   `pij-rs` into `$(npm prefix -g)/bin`, next to the `pij` shim.
3. **`just omp-install`** — install OMP from the governed registry when absent,
   then `just link` (refuses linked worktrees; links only `pij` into
   `~/.omp/agent/extensions/`, `~/.omp/agent/{mcp.json,models.yml}` to this
   repo's `.omp/`, and the native Copilot extension) and `just omp-doctor`.
4. **`just pij-skill-link-global` + `npm link`** — the `/pij` skill and the
   `pij` CLI shim, machine-wide.
5. **`just doctor`** — read-only verification of every surface above, plus the
   optional Copilot native check.

## The recipe surface

`just` with **no recipe** lists everything. Day-to-day recipes:

| Recipe | Does |
|--------|------|
| `just typecheck` | `tsc --noEmit` via `npm run typecheck` |
| `just lint` / `just format` | Biome check (errors + warnings) / auto-fix |
| `just test [path]` | Run vitest; optionally scope to a file/pattern |
| `just rust-check` | `Cargo.lock` current, `cargo fmt --check`, clippy `-D warnings`, `cargo test` |
| `just smoke` | tmux-driven end-to-end smoke (Driver SDK) |
| `just self-check` | The composite gate (below) |
| `just bounce-rs` | Rebuild `pij-rs` and restart the daemon |
| `just rs-autostart` / `just rs-autostart-off` | macOS LaunchAgent for the daemon |
| `just link` / `just unlink` | Guarded canonical OMP + Copilot links |
| `just doctor` / `just omp-doctor` / `just where` | Read-only diagnostics |
| `just update-omp` | Reinstall OMP from the governed registry and re-link |
| `just pij <args>` | Run the pij CLI through the locked local wrapper, no global install |
| `just release-age-probe` | Prove locked install, fresh-resolution refusal, and audit visibility |

## npm resolution boundary

The root [`.npmrc`](../../.npmrc) fixes four settings:

```ini
replace-registry-host=npmjs
prefer-online=true
min-release-age=7
audit=true
```

The registry defaults to public npm (`https://registry.npmjs.org/`). On a machine
that must resolve through a mirror or proxy, export `PIJ_NPM_REGISTRY=<url>`; the
governed helpers and `just` recipes use it in place of the default.

The typed helper at `harness/scripts/release-age-policy.ts` removes inherited
registry, lock-host replacement, online, age, and `before` overrides
case-insensitively, then supplies the governed lowercase values without mutating
its caller. The fail-closed
runner at `harness/scripts/npm-resolution-run.ts` applies that environment to
every fresh resolution pij performs (the OMP and bun installs).

Locally locked tools use `node_modules/.bin/*` or a checked-in wrapper instead
of opportunistic `npx` resolution. In particular, `just pij` executes
`node harness/scripts/pij-cli.cjs`; missing local `tsx` fails instead of being
downloaded at runtime.

The governed registry (`PIJ_NPM_REGISTRY`, default public npm) is the only live
read authority. `replace-registry-host=npmjs` rewrites npmjs lock-resolved hosts to
that authority, and `prefer-online=true` revalidates stale client metadata against
it. If the registry omits an exact lock
target or advertises an unusable tarball, npm fails closed: pij does not delete
the cache, rewrite the lock, retry another registry, or return a success-shaped
fallback.

A successful `npm ci` proves only that the frozen lock installs. It is not
fresh-resolution evidence. npm/cli
[#9005](https://github.com/npm/cli/issues/9005) currently makes the project
`min-release-age` conflict with npm's internally derived `--before` during nested
git preparation. Root lock replay therefore uses
`npm ci --min-release-age=null`: the CLI `null` clears the inherited value for
that frozen operation. The exception is wired only into `just install` and the
CI lock-replay step. Never use it with `npm install`.

CI runs `npm audit --audit-level=high` as a hard gate.

Run the isolated proof explicitly:

```bash
just release-age-probe
```

It creates isolated HOME, cache, user/global config, project, tarball, and local
registry state. Native npm must derive a `before` date approximately seven days
earlier and refuse a fixture version published at probe time without any raw
age argument. The same run creates a fixture lock, proves exact replay still
fails after the local proxy removes its artifact, observes local audit JSON,
verifies the repository manifests were unchanged, captures every subprocess
result, and removes its temporary root. It does not download the root lock or
use the caller's npm state.

There is no generic age-zero recipe. The only exception is the exact root
lock-replay command above; every fresh `npm install` remains at seven days.

The deeper regression fixture is
`harness/scripts/npm-resolution-policy.integration.test.ts`. It uses mutable
local proxy and upstream servers with real tarballs and request logs to prove
online stale-cache recovery, proxy-truth changes, missing and corrupt artifacts,
governed replacement of an upstream-host lock URL despite a caller `never`
override, exact-lock absence, age refusal versus a test-only age-zero mutation,
and zero governed requests to the fixture upstream.

## The gate: `just self-check`

Before declaring any task done — or before ship — run the composite gate.
Agents **must** run this and never compose the steps by hand:

```bash
just self-check
```

It runs, in order: **`local-path-check` → `lockfile-allowlist` → `typecheck` →
`lint` → `test` → `copilot-native-test` → `rust-check` → `smoke` →
`pij-commit-trailers`**. The portability sensor rejects user-specific absolute
home paths in operational source/config before they reach another machine; the
commit-trailer scan is advisory.

## The engineering harness: `harness boot` / `checks` / `doctor`

pij has adopted the ai-substrate engineering harness. Two verbs matter
day-to-day (see [`AGENTS.md`](../../AGENTS.md) § Harness tooling and
[`.harness/engineering-harness.md`](../../.harness/engineering-harness.md)):

- **`harness boot`** — fast readiness proof (typecheck + test).
- **`harness checks`** — the full ship/done gate: the same signal inventory as
  `just self-check`, but it runs **all** sensors and reports a per-sensor
  verdict, so one invocation surfaces every failure. `harness checks --quick`
  skips the heavy Rust and smoke stages for a fast static + unit gate.
- **`harness doctor`** — audits what the harness loaded.

`harness checks` and `just self-check` are the same composite from two front
doors; use whichever you prefer. New back-pressure sensors are added under
`.harness/extensions/checks/` so this one verb stays the single "are we done?"
gate.

## Smoke = the tmux Driver SDK

`just smoke` runs `harness/scripts/smoke.ts`, a thin adapter over the typed
**Driver SDK** at `harness/driver/` (`Scenario` / `Step` / `Session`). It runs
every `.omp/extensions/<name>/smoke.ts` plus the governance round trip against a
private `pij-rs` daemon in tmux, so it requires `tmux` on `PATH`. Author rich scenarios against the SDK directly; the
smoke script is just the default entry point.

## See also

- [`skills.md`](skills.md) — where skills live and how they install.
- [`AGENTS.md`](../../AGENTS.md) — the full agent rules (P1–P10, workflow).
- [`RUNBOOK.md`](../../RUNBOOK.md) — the operational runbook.
