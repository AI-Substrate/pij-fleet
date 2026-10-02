# pij Runbook

## Boot

```bash
just install        # deps, pij-rs, OMP + extension links, skill, CLI shim
just bounce-rs      # build and (re)start the pij-rs daemon
just self-check     # validates the harness still works end-to-end
```

If `self-check` fails, fix it before doing anything else. **The harness IS
the product.** `just doctor` is the read-only first diagnostic when something
has drifted.

## Daemon lifecycle

| Task | Command |
|---|---|
| Rebuild + restart after any Rust change | `just bounce-rs` |
| Health | `pij-rs ping` |
| Start at login (macOS LaunchAgent) | `just rs-autostart` / `just rs-autostart-off` |
| Logs | `~/.pij-rs/daemon.log` |
| Where the live surfaces resolve | `just where` |

The daemon runs from the `pij-rs` binary linked by `just rs-install`; there is no
hot reload. A daemon change takes effect only after `just bounce-rs`.

## Dependency release-age policy

The committed [`.npmrc`](.npmrc) sets npm's native `min-release-age=7`
(**days**) and keeps `audit=true`. The same seven-day environment applies to
pij-owned fresh resolutions (the OMP and bun installs in `just omp-install` /
`just update-omp`) through `harness/scripts/npm-resolution-run.ts`.

`npm ci` proves the committed lock remains installable; it does **not** prove a
fresh resolution would accept the same versions. npm/cli
[#9005](https://github.com/npm/cli/issues/9005) makes the inherited relative age
conflict with npm's derived `--before` during nested git preparation, so root
lock replay uses the only approved compatibility form:

```bash
npm ci --min-release-age=null
```

The `null` CLI value clears the inherited project setting for this frozen-lock
operation. It is not a fresh install bypass, must never be used with
`npm install`, and is the only age-zero path encoded in normal recipes or CI.
Run the independent proof with:

```bash
just release-age-probe
```

The probe replays the committed lock with that scoped exception, derives a
`before` date approximately seven days earlier from the committed `.npmrc`,
refuses a deterministic newly-published local registry fixture without a raw age
argument, and captures `npm audit --json` separately.

The registry defaults to public npm. On a network where `registry.npmjs.org` is
unreachable, export `PIJ_NPM_REGISTRY=<mirror url>`.

## Iterate on the OMP extension

```bash
omp                 # from the repo root; the extension is linked by `just link`
```

In the TUI, after edits: `/reload`. There is no file watcher — the reload is
manual on purpose. Keep `just typecheck` and `npm run test:watch` running in
other panes.

## OMP peer delivery

OMP task children do not own pij inboxes or peer controls; independently hosted
OMP roots (including headless roots) still register. See the
[API contract](docs/how/pij-rs-api.md) for delivery and receipt semantics.

Opt-in real-client proof (owned daemon, tmux server, and scratch workspace):

```bash
PIJ_STEP_ON_REAL=1 just step-on-probe --harness omp --state idle
PIJ_STEP_ON_REAL=1 just step-on-probe --harness omp --state bash --model github-copilot/gpt-5-mini --thinking minimal
PIJ_STEP_ON_REAL=1 just step-on-probe --harness omp --state subagents --model github-copilot/gpt-5.4-mini --thinking low
```

Busy proof requires the original parent session, its nonce reply before remaining
work completes, an intact draft, and the matching inbox ACK. `--thinking` overrides
ambient OMP effort so the probe measures delivery instead of prolonged reasoning.
The OMP probe uses a fixed, recorded system prompt to isolate delivery from ambient
coding instructions. Busy context must be a direct pij entry at the original tool
boundary, not a child-result notice or a follow-up after a terminal response.
Unreadable redraw frames are reacquired within one second and counted in the
receipt; empty or changed draft text still fails immediately.

## Smoke

```bash
just smoke           # every .omp/extensions/<name>/smoke.ts via the Driver SDK
```

Requires `tmux`.

## When something hurts

1. Open `docs/difficulties.md`, append a row (D-NNN).
2. If the fix is small/surgical (fits the current task scope), **encode
   it now** (recipe, lint rule, helper, check). Do not just document it.
3. Otherwise, file a `stretch:` row and link the difficulty.

## Where things are

| What | Where |
|---|---|
| Daemon + native CLI (Rust workspace) | `crates/` |
| `pij` CLI shim | `harness/scripts/pij-cli.cjs` |
| OMP extension | `.omp/extensions/pij/` |
| Copilot native extension | `.copilot/extensions/pij/` |
| Claude Code hook + statusline scripts | `harness/scripts/claude-*-pij.sh` |
| OMP model overrides / MCP config | `.omp/models.yml`, `.omp/mcp.json` |
| Link script | `harness/scripts/link-global.ts` |
| Driver SDK (typed smoke) | `harness/driver/` |
| Smoke runner | `harness/scripts/smoke.ts` |
| Skills | `skills/` |
| Engineering harness | `.harness/` |
| Difficulty ledger | `docs/difficulties.md` |
| Velocity log | `docs/velocity.md` |
| BIO contract | `docs/project-rules/harness.md` |
