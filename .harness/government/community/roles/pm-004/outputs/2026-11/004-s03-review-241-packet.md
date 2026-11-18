# Packet: verdicts on PR #241 (workstream 004 s03, plan 229: the daemon side of the Copilot cache proxy)

From pm-004 (pij-outstanding-silkworm), 2026-11-18. One packet for three seats. Each of you gives **one** verdict, for your role: **reviewer**, **security** or **performance**. Read your role's standing packet first: `~/pi-hacking/pij-governance/.harness/government/community/roles/<role>/role.md`, then your role memory (`pij directory memory <role> --project pij-fleet`). Earlier cache-proxy reviews left notes there.

## What to review
- PR: https://github.com/AI-Substrate/pij-fleet/pull/241
  - **Head 872fafab72747009c9d3b3c61635e130bc6220d3**: 37 files, +2632 -81, rebased on main d3919152 (#239, the sidecar, is merged).
  - Worktree (detached, read-only, frozen): `/Users/jordanknight/pi-hacking/review-241-872fafa`.
- Plan and proof: `docs/plans/229-cache-proxy-gate/` (plan, backpressure, `assets/proof.md` with receipts from the real sidecar).
- Design, now on main:
  - `docs/project/blueprints/003-copilot-cache-proxy.dd.md`: components c-spawn-gate, c-seat-proxy-state, c-proxy-watch and c-doctor-row, plus the c-status-route contract it reads.
  - PRD 004: req-01, 04, 05, 08, 09, 11, 12 and 13.
  - Matrix: `~/pi-hacking/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.
- What it does:
  - **Gate.** In `launch_seat_locked`, a Copilot seat runs plain on `--no-cache-proxy`, on a non-Claude model, on a not-ready status (one loopback GET, 1 s timeout), or on a base id missing from status `models`.
  - **Env.** Otherwise the gate sets `COPILOT_PROVIDER_TYPE`, `_BASE_URL`, `_MODEL_ID`, `_MAX_PROMPT_TOKENS` and `_VISION=true`.
  - **Seat record.** `SeatDescriptor.cache_proxy` comes from migration 0047. It appears in the spawn receipt and on directory rows.
  - **Watch.** A 30 s status read sends one FYI per state change to each distinct live prime. The first read after a daemon start is silent, unless the proxy isn't ready while proxied seats are live.
  - **Health.** `/health.cache_proxy` reports the proxy's state.
  - **Doctor.** `cache-proxy:` and `cache seats:` rows.
  - **Revive.** Revive re-runs the gate and keeps an opt-out.
  - **Docs.** The PR also carries the delivery edits from `pij delivered 239`:
    - s02 marked complete;
    - delivered_by #239 on 8 requirements;
    - req-01 added to s03.

## Builds: none
**Don't build.** CI on the exact head is this PR's build. Read it with `gh pr checks 241` and `gh run view <id> --log`, and wait for it if it's still running.

One known flake: `pij-tmux adapter separate_adapters_cannot_both_reserve_one_pane`. It also failed once on #238, a docs-only PR. This branch doesn't touch crates/tmux. If CI fails only there, say so and don't block on it.

Build only if a finding can't be settled otherwise. Then ask me first, and use `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <args>` scoped with -p.

Scratch dirs and a private daemon only. Never touch the prod daemon, port 7471 or a LaunchAgent.

## Check by role
**reviewer (correctness):**
1. The gate's plain/proxied decision is right for every input:
   - harness (OMP, Claude and Codex never touched: row 20);
   - model (`claude-` prefix; no `--model` means plain);
   - `-1m` stripping;
   - not ready, timeout or refused;
   - base id missing from `models`.

   The env set is exact.
2. Migration 0047 is additive, and NULL reads as plain (row 18). The descriptor round-trips through the store, the receipt and directory rows.
3. Watch transitions:
   - one notice per real change, to each distinct live prime (deduped);
   - the first-read rule;
   - `off` worded as "new spawns plain, existing seats unaffected";
   - unreachable and back.

   No repeats (rows 7, 21; F192). Check the state machine for a flapping or missed transition.
4. Revive re-runs the gate, and `--no-cache-proxy` survives revive.
5. The docs edits are correct ddocs. Delivers matches what's built. All 14 reqs are `shipped`: say whether that's right before the s04 live acceptance.
6. Mutation-check the guards the tests rely on.

**security:**
1. The daemon never receives, logs or stores a token: it reads only the status route. The status struct has no secret field.
2. Env injection: model and base-id strings reach the `env` argv of the spawn. Check quoting, and that a crafted `--model` or status `models` key can't inject extra env or args.
3. The status URL is loopback-only: `PIJ_CACHE_PROXY_URL` is a test override read once at start. Can a seat or request redirect the gate?
4. Notices and doctor output carry no token or account secret.
5. Public-repo rules: fixtures, logs, docs.

**performance (cost, AGENTS.md):**
1. Name each loop, poll and launch with its rate and per-seat scaling. Expected (blueprint k-4, k-5):
   - one loopback GET per Copilot spawn;
   - one loopback GET every 30 s;
   - 0 launches;
   - no per-seat polling.

   The live-seat proxied/plain split in `/health` is computed on the tick: confirm it adds no launch or per-seat I/O.
2. Receipts: spawn counter 24 → 28 → 30, with only existing tmux ops and no cache-proxy op. Measure the live daemon's spawns twice, a known interval apart (read-only), as a baseline.
3. The 30 s timer can't stack or overlap when a GET is slow (1 s timeout).

## Output (do not skip)
1. Publish with the writer (`pij output verdict`) and post `gh pr comment 241`. Start with "<role>: pass|pass-with-notes|block (head 872fafab72747009c9d3b3c61635e130bc6220d3)". Give each finding a severity and a concrete fix.
2. **Send me the report** with the pij_send tool: the verdict, findings by severity, and the verdict path. **The job isn't done until that send has gone.**

Two review rounds at most. Never commit or push. Never type into a pane. Never read credential files or print a token. Never use a question UI; ask me in plain text.
