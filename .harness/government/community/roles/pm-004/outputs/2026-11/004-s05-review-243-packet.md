# Packet: verdicts on PR #243 (workstream 004 s05, plan 230: F204, F205 and count-only notices)

From pm-004 (pij-outstanding-silkworm), 2026-11-18. One packet for three seats. Each of you gives **one** verdict for your role: **reviewer**, **security** or **performance**. Read your role's standing packet first (`~/pi-hacking/pij-governance/.harness/government/community/roles/<role>/role.md`), then your role memory (`pij directory memory <role> --project pij-fleet`). Earlier cache-proxy reviews (#239, #241, #242) left notes there.

## What to review
- PR: https://github.com/AI-Substrate/pij-fleet/pull/243. **Head ed6462dbe3ce24e453ebeb47ad6ce8503721611d**: 19 files, +1745 -144, rebased on main 48437352.
- Frozen worktree (detached, read-only): `/Users/jordanknight/pi-hacking/review-243-ed6462d`.
- Plan and proof: `docs/plans/230-cache-proxy-watch/` (the plan, its backpressure, and `proof.md`, which has the live receipts and the 7/7 mutation receipts).
- Defects:
  - **F204:** the false outage. At 14:56 one 1-second status read timed out while the sidecar was up, and the outage notice named 43 seats. Root cause: `status()` waited on `run_check`'s upstream GETs once its cache expired.
  - **F205:** the stale 402 latch. A 402 from a request sent with account A's token latched `quota_exhausted` for account B after the switch back. Evidence: s04 row 12 in `docs/plans/004-acceptance/receipts.md`.
  - **Notice noise (F192 class):** every notice listed every proxied seat.
- The fix:
  - **Sidecar:** `/__pij/status` answers from cache and starts a single-flight background check. Startup and reload still wait for the check.
  - **Sidecar:** the F205 stale rule. A 402 latch, and a 2xx clear, apply only if the request's account and generation match the current holder.
  - **Sidecar:** `checked_at_ms` is added to the status output.
  - **Daemon:** a refusal is an outage at once. Timeouts count as "slow", and only 2 consecutive failed reads make an outage. `slow` is reported separately in /health and doctor.
  - **Daemon:** on a timeout, the gate retries once.
  - **Daemon:** notices carry a count and point to `just doctor` instead of listing seats.

## Builds: none
CI on the exact head is the build: `gh pr checks 243`, `gh run view <id> --log`. Build only when a finding can't be settled any other way. Ask me first, then use `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <args>` with `-p` scoping. Use a private daemon and scratch dirs only, and never port 7471, the LaunchAgent or the prod daemon: live seats depend on them.

## Check by role
**reviewer (correctness):**
1. The status route never awaits upstream after startup. Background checks are single-flight: two concurrent reads start one check. The F002 latch (clears only on a 2xx, the reset date or a reload) and the F001 install rule still hold.
2. F205: the stale rule covers both the 402 latch and the 2xx clear, keyed on the account and the generation the request actually carried. Look for any path that still attributes a response to the current holder.
3. The watch state machine:
   - refusal: 1 notice;
   - one timeout: 0 notices;
   - two consecutive timeouts: 1 notice;
   - timeout, then ready, then timeout: 0 notices;
   - recovery: 1 notice;
   - the first-read rule;
   - `off` keeps its own wording.

   Look for any flap or missed transition.
4. Gate retry: bounded, ends plain with the reason, and doesn't double the spawn latency in the normal case.
5. Docs: blueprint c-status-route, c-readiness and c-proxy-watch match the code. s05 is in the workstream. The F-IDs are F204 (false outage) and F205 (stale 402), plus F192-class notice noise.
6. Mutation-check the guards. The author's receipts: 7/7 killed.

**security:**
1. The background check can't be triggered so as to amplify upstream calls: one check at a time, and its rate is bounded by the cache TTLs.
2. `checked_at_ms` and the new `slow` fields leak nothing sensitive.
3. The F205 rule can't be gamed so that a quota state attaches to the wrong account.
4. Notices name no seat ids (less public exposure in any copied logs).
5. Public-repo rules for the fixtures and the proof docs.

**performance (cost, per AGENTS.md):**
1. Name every loop and launch. Expected:
   - the background check replaces the inline check, so no new timer;
   - the watch stays at one loopback GET every 30 s;
   - the gate retry is one extra loopback GET, and only on a timeout;
   - 0 new launches.
2. Receipts: spawns 63 → 72 across the run (existing tmux ops only), and 72 → 72 over 60 s at steady state with 5 seats.
3. Concurrent status reads under a stall don't pile up tasks or connections.

## Output (do not skip)
1. Publish with the writer (`pij output verdict`) and post `gh pr comment 243`. Start with "<role>: pass|pass-with-notes|block (head ed6462dbe3ce24e453ebeb47ad6ce8503721611d)". Give each finding a severity and a concrete fix.
2. **Send me the report with the pij_send tool:** the verdict, the findings by severity, and the path. The job isn't done until that send has gone.

Two review rounds at most. Never commit or push. Never type into a pane. Never read credential files or print a token. Never use a question UI; ask me in plain text.
