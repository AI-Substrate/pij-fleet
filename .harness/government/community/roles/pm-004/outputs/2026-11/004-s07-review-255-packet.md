# Packet: verdicts on PR #255 (workstream 004 s07, plan 232: proxied tool-list flips and the per-seat TTL)

From pm-004 (pij-outstanding-silkworm), 2026-11-19. One packet goes to three seats. Each seat gives **one** verdict, for its role: **reviewer**, **security** or **performance**. Read your role's standing packet first (`~/pi-hacking/pij-governance/.harness/government/community/roles/<role>/role.md`), then your role memory (`pij directory memory <role> --project pij-fleet`).

## What to review
- PR: https://github.com/AI-Substrate/pij-fleet/pull/255. **Head c68ac8f7fb8adb84af2fe0372276cef51686c802**: 15 files, +1581, on main 408418a1.
  - Frozen worktree: `/Users/jordanknight/pi-hacking/review-255-c68ac8f`.
- **One more commit is expected by 04:40Z.** It adds the after-window re-run (≥1h of post-restart data, split by process start) and checks AC5. When I send the new head, `git range-diff` it against c68ac8f7. Grade the final head; it should differ only in data and output files.
- **The PR is docs plus Python analysis scripts. It contains no product code.**
  - The fix itself (`toolSearch=false` in the user-wide `~/.copilot/settings.json`) was applied by the prime on Jordan's go at 03:27:46Z. It is NOT in this PR.
- Contents:
  - plan 232: `docs/plans/232-proxy-tool-flips/` (plan, backpressure, tasks, original ask);
  - assets: `flip_window.py`, `sim232.py`, and their outputs `canary-sessions.md`, `flip-window.md`, `sim232.md`;
  - workstream 004 gains s07, plus a changelog row;
  - `docs/difficulties.md` gains D-232-1 to D-232-4.
- Claims under test:
  - Cause: on the BYOK route, Copilot's client-side generic tool search adds and removes deferred tools, so the tools prefix changes and the whole prompt gets re-written. Shown by usage records: 87% of flips re-wrote more than half the prompt, against 0.3% of requests without a flip.
  - Canary under `toolSearch=false`: 0 count flips over 18 requests. Residual: the first `extensions_reload` makes a token-only flip.
  - Re-pricing reconciles to billed exactly: 661,994 AIU over 43,324 records.
  - Result: the proxy goes from 10.7–27.6% dearer than plain to 6.9–25.7% cheaper.
  - Per-seat TTL policies all lose (+6% to +26%), and a hindsight bound saves only 0.9–3.8%. Verdict: keep 1h for everyone and build nothing.

## Builds: none
CI on the exact head is the build. Scripts may be **run read-only** to check their numbers: `python3 docs/plans/232-proxy-tool-flips/assets/sim232.py` in the worktree. They read `$HOME/.copilot/session-state/*/events.jsonl`, usage fields only. Do not run anything that writes outside a scratch dir.

## Check by role
**reviewer (correctness):**
1. Does `sim232.py`'s pricing really reconcile to billed (the classes × each record's listed prices)? Do the counterfactual's two parts follow from the canary's evidence?
   - (a) Removing flips: only flips in the same process, with no compaction between, inside the previous TTL.
   - (b) +12,933 prefix tokens: read on a cache hit, written on a full miss.
2. The per-seat TTL sim respects the never-upgrade rule. The hindsight bound is computed correctly.
3. The conclusions don't go past the evidence. Thin cells are flagged (the fixed 5–60-minute cell has n=103; the after-window is thin until the re-run).
4. Docs: s07 is in the workstream, Delivers matches it, the req-01/req-07 wording is accurate, and the ddocs are valid and current.
5. Mutation-check the key computations by reasoning. Where a number doesn't reproduce, run the script.

**security:**
1. The scripts read only the usage fields from `session.usage_record`. They never read message content, credential files or `~/.copilot/config.json` secrets.
2. No committed output carries session text, prompts, tokens, account secrets or private paths (the scripts use `$HOME`). pij-fleet is public.
3. The scripts can't be pointed at writing outside their output files.

**performance (cost):**
1. The PR adds no loop, poll, timer, hook or launch. Confirm this from the diff.
2. Audit the cost claims as model cost: AIU per token class, no assumed TTL (only the recorded `cachePolicyTtlSeconds`), and every figure with its receipt.
3. The restart-cost finding: one whole-prompt write per restarted seat (7c8a587b re-wrote 300,980 of 335,398 tokens). Is the advice sound: restart big-context seats only when they would cold-wake anyway?

## Output (do not skip)
1. Publish with the writer (`pij output verdict`) and post `gh pr comment 255`. Start with "<role>: pass|pass-with-notes|block (head <full sha>)". Give each finding a severity and a concrete fix.
2. **Send me the report with the pij_send tool**: verdict, findings by severity, and the path. The job isn't done until that send has gone.

At most two review rounds. Never commit or push. Never type into a pane. Never read credential files or print a token. Never use a question UI; ask me in plain text.
