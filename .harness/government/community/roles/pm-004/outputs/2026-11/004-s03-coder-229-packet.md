# Packet: coder, workstream 004 s03 (plan 229: the daemon side of the cache proxy)

From pm-004 (pij-outstanding-silkworm), 2026-11-18. You hold `coder/229` for workstream 004 s03. Load the pij skill and the builder skill. Read these in this order:
1. Blueprint 003 (`/Users/jordanknight/pi-hacking/ws004-docs/docs/project/blueprints/003-copilot-cache-proxy.dd.md`; PR #238, not merged yet).
2. PRD 004 (same folder, `prds/004-copilot-cache-proxy.dd.md`).
3. Workstream 004, step s03.
4. The acceptance matrix: `/Users/jordanknight/pi-hacking/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.
5. The rulings: `handover/ruling-2026-11-18-copilot-cache-proxy.md` in the same folder.

## What you build
Blueprint components c-spawn-gate, c-seat-proxy-state, c-proxy-watch and c-doctor-row:
- **Gate and env.** In `launch_seat_locked` (`crates/daemon/src/http/mod.rs`), Copilot harness only:
  - The seat runs plain if `--no-cache-proxy` was given (a new flag on `Command::Spawn` and a field on `SpawnRequest`), if the model isn't Claude, or if `GET http://127.0.0.1:7471/__pij/status` (1 s timeout) isn't `ready`.
  - Otherwise `SpawnPlanInput` carries the proxy base URL, and `build_spawn_plan` (`crates/harnesses/src/spawn.rs`) adds `COPILOT_PROVIDER_TYPE=anthropic` and `COPILOT_PROVIDER_BASE_URL=…`.
  - OMP, Claude and Codex seats are never touched (row 20).
- **Seat state and receipt.**
  - A `cache_proxy: {state: proxied|plain, reason, fix}` field on `SeatDescriptor`. If it is missing, the seat reads as plain (row 18).
  - It shows in the spawn receipt (`SpawnResponse`) and on directory holder rows (`crates/daemon/src/community/directory.rs`, rendered in `render_directory`, `crates/cli/src/main.rs`).
- **Watch and notices.**
  - New `crates/daemon/src/cache_proxy.rs`: one loopback GET of the status route every 30 s (0 launches).
  - On a change of `state`, or when the route becomes unreachable: **one** notice per change to each prime, through `notify_prime` (`crates/daemon/src/http/routes.rs`). It names the proxied live seats and the fix. Never a repeat (rows 7, 21; F192).
  - `/health` gains `cache_proxy` (the last status and counters), so `pij-rs --json ping` shows it.
- **Doctor.** The justfile `doctor` recipe gains a `cache-proxy:` row from `pij-rs cache-proxy status`, which lane s02 builds; it includes the second-human-account warning.

## The contract you build against (s02 builds the other side in parallel)
- Blueprint c-status-route:
  - `GET /__pij/status` returns `{ready, state, reason, fix, account, upstream_host, quota: {usable, reset}, counters: {requests, refreshes, gh_launches, upstream_401, upstream_402}, pid, since}`.
  - `state` is one of `ready`, `off`, `no_account`, `gh_missing`, `no_token`, `quota_exhausted`, `upstream_unreachable`.
- Deserialize it into your own struct in `crates/daemon/src/cache_proxy.rs`. Don't depend on the s02 crate, so the two lanes don't block each other.
- In tests, stand up a stub status route (axum on an ephemeral port) to drive each state. A connection refused or a timeout means plain, with reason "proxy not listening".
- The proxy URL comes from config. The default is `http://127.0.0.1:7471`, with an env override `PIJ_CACHE_PROXY_URL` for tests only, read once at startup.
- s02 merges first. You rebase on it, and before the PR your live proof uses its real sidecar.

## Where and how
- Worktree: `/Users/jordanknight/pi-hacking/fleet-229-cache-proxy-gate`, branch `coder/229-cache-proxy-gate` (from origin/main 84ef0774).
  - Your seat runs in the main checkout (OMP can't start in a linked worktree), so work there by absolute path.
- Plan: claim is done (plan 229). Write `docs/plans/229-cache-proxy-gate/` with `/builder`: plan, then tasks, then implement.
- **Builds go through `just shared-build` only, niced, one at a time:**
  - `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <cargo args>`.
  - The s02 coder shares this target, and the lock serialises you.
  - One build per change; use -p scoping.
  - If shared-build refuses on the disk floor, stop and tell me. Never work around it.
- **Cost (AGENTS.md):** your loop is the 30 s watch (blueprint k-5) and the per-spawn GET (k-4).
  - Measure `pij-rs --json ping | jq .data.spawns` before and after on your **private** test daemon.
  - Record 0 new launches.
  - Any new launch site needs `pij_core::spawns::record`.
- Tests use scratch repos and a private daemon. Never use the live governance branch or the prod daemon.
- Proof before the PR: matrix rows 1, 3 to 8, 12, 13 and 18 to 21, against your private daemon and s02's sidecar on a test port.
  - Live-fleet receipts are s04's job.
  - Record each row as: command, observed value, seat ids.
- Gates before the PR: scoped `just rust-check` (through shared-build), `just lint` from the main checkout, `just local-path-check`, `just lockfile-allowlist`, `just ddocs-check`. Then `harness checks --quick`.
- Open the PR against main.
  - `Delivers:` workstream 004 s03; req-04, req-05, req-08, req-09, req-11, req-12.
  - Send me the number, head, checks and the spawn-counter readings. Reviews are Copilot gpt-6.1-sol seats (reviewer, security, performance), two rounds at most.
- After the PR is out for verdicts, push nothing unless a verdict asks, and tell me first. Never use a question UI; ask me in plain text.

Commit trailers: `Pij-Seat: <your seat id>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 229-cache-proxy-gate`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit. Reply with `pij send pij-outstanding-silkworm --body-file - <<'PIJ'` … `PIJ`.

Never:
- restart, bounce or deploy the prod daemon;
- install either LaunchAgent;
- type into any pane you didn't create;
- read credential files or print a token;
- `git add -A`, `--no-verify`, or push `refs/notes/*`;
- `npm install`.
