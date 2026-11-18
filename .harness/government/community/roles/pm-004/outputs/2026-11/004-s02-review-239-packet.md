# Packet: verdicts on PR #239 (workstream 004 s02, plan 228: the Copilot cache-proxy sidecar)

From pm-004 (pij-outstanding-silkworm), 2026-11-18. Three seats get this one packet, and each one gives **one** verdict for its own role: **reviewer**, **security** or **performance**. Before anything else, read your role's standing packet: `~/pi-hacking/pij-governance/.harness/government/community/roles/<your role>/role.md`.

## What to review
- PR: https://github.com/AI-Substrate/pij-fleet/pull/239.
  - **Head 91e7d311e29f577235f4c859d19e0e888955b95c**: 33 files, +4573.
  - Worktree (detached, read-only, frozen): `/Users/jordanknight/pi-hacking/review-239-91e7d31`.
- The plan and its live-row receipts:
  - `docs/plans/228-cache-proxy-sidecar/` (plan.dd.md, assets/backpressure.dd.md, assets/evidence/live-rows.md).
  - The PR body has the same receipts.
- Design sources. They are on PR #238, not on main yet, so read them at `/Users/jordanknight/pi-hacking/ws004-docs/docs/project/`:
  - blueprint 003 (`blueprints/003-copilot-cache-proxy.dd.md`);
  - PRD 004 (`prds/004-copilot-cache-proxy.dd.md`).
  - Acceptance matrix: `~/pi-hacking/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.
  - Rulings: `handover/ruling-2026-11-18-copilot-cache-proxy.md` in the same folder.
- What the PR does:
  - It adds a new crate, `crates/cache-proxy`, and `pij-rs cache-proxy` (run / status / account / off / on).
  - It listens on loopback only, rewrites `cache_control` to `ttl:"1h"`, sets `X-Initiator: agent` on tool-result continuations, and maps `-1m` model ids to the base id.
  - It holds a `gh auth token --user` token in memory and refreshes it only on a 401 (single-flight).
  - It checks readiness through `copilot_internal/user` (endpoint, quota) and `/models`.
  - The status and reload routes are at `/__pij/status` and `/__pij/reload`.
  - It adds LaunchAgent recipes.

## Builds: none
**Don't build.** One build per PR, and this PR's build is CI on the exact head (rust-lint, rust-test, check 22/24, delivers). Read the CI logs: `gh pr checks 239` and `gh run view <id> --log`. Build only if a finding can't be settled any other way. Then ask me first, and use `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <args>` scoped with -p. Use scratch dirs only; never the prod daemon. Never start a sidecar on port 7471.

## Check by role
**reviewer (correctness):**
1. The status route matches blueprint c-status-route exactly, plus the type rules in the plan's clarifications: `since` in epoch ms, null `account`/`upstream_host`, empty `reason`/`fix` when ready, and the failure-to-state mapping. Lane s03 (plan 229) is built against it.
2. A 401 triggers one single-flight refresh and one retry. A failed refresh keeps the held token and latches `no_token` until a reload, with no `gh` loop. Since the fix, a successful refresh never shows `upstream_unreachable`.
3. Pass-through is faithful: SSE streams untouched, the request body otherwise byte-identical, the `-1m` mapping applied only to `model`, and the TTL rewrite applied only to `cache_control`.
4. `off` turns `ready` false but keeps serving seats already proxied. `account` and `on` write the config file, then reload.
5. Mutation-check the guards the tests rely on (the rewrite, single-flight, the 401 latch, the dscl parser).

**security:**
1. The token never appears in logs, files, argv, the status JSON or error bodies. Check every log line and error path, and look at the grep receipt.
2. `PIJ_GH` and every test-only knob (stale token and similar) are read once at startup and can't be set per request.
3. The loopback listener has no authentication (single human account, ruling 3). Check the bind address. Check the Origin/Host 403 that happens before forwarding (described in the PR body under "For security"); try to get past it with DNS rebinding or `Host: localhost.` variants.
4. Upstream: requests go only to `endpoints.api` from `copilot_internal/user`. A forged status or reload can't redirect them, and `reload` takes no body.
5. The public-repo rules: no secrets in fixtures, the lockfile stays on registry.npmjs.org, and the `gh` launch uses an absolute path.

**performance (cost: AGENTS.md "Resource cost is a correctness property"):**
1. Name every loop, poll, timer and launch, with its rate and how it scales per seat. Expected (blueprint costs k-1 to k-3):
   - 0 launches per request;
   - 1 `gh` per sidecar start, plus at most 1 per 401 burst;
   - 1 `copilot_internal/user` GET and 1 `/models` GET per 10 minutes per account.
2. The `gh` launch site records through `pij_core::spawns::record`, and spawn_sites passes in CI.
3. Check the receipts: daemon spawns 475 → 475 over 20 s; `just spawn-rate --seconds 20` 96.1/s with `daemon_children` empty; row 17 (10 clients: requests 0 → 10, `gh_launches` 1 → 1); row 9.
4. Look for any hidden per-request allocation or copying of large bodies where a stream would do (prompts are up to 1M tokens).

## Output (do not skip)
1. Write a verdict file at `~/pi-hacking/pij-governance/.harness/government/community/roles/<your role>/outputs/2026-11/pr-239-verdict.md`. Write it to disk; don't commit (pm-004 commits it).
2. Post it with `gh pr comment 239 --body-file <file>`, starting "<role>: pass|pass-with-notes|block (head 91e7d311e29f577235f4c859d19e0e888955b95c)". Give each finding a severity and a concrete fix.
3. **Send me the report** with `pij send pij-outstanding-silkworm --body-file - <<'PIJ'` … `PIJ`: verdict, findings by severity, and the file path. **The job isn't done until that send has gone.**

Two review rounds at most. Never commit or push. Never type into a pane. Never read credential files or print a token. Never use a question UI; ask me in plain text.
