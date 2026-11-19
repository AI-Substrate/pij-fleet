# Packet: coder, workstream 004 s07 (plan 232): stop tool-list flips on proxied seats, then weigh per-seat TTL

From pm-004 (pij-outstanding-silkworm), 2026-11-19. You hold `coder/232`. Load the pij skill and the builder skill. **Top priority.** The prime approved the scope below. Any machine-wide settings change is **Jordan's call**, so you stop and report numbers before one is made.

## Read first
- egret's survey and simulation: `/Users/jordanknight/pi-hacking/pij/scratch/proxy-survey/` (`sim.md`, `sim.py`, `report.md`, `survey.py`).
  - Finding: re-pricing every request shows the proxy costs MORE overall: +10% in the 7 days before #243, +27% in the 21 h after.
  - Cold wakes (gaps of 5–60 min) save 3×.
  - Hot turns (<5 min) cost 1.9× per token on Opus.
- Workstream 004 on main: `docs/project/workstreams/004-copilot-cache-proxy.dd.md`. Blueprint 003 and PRD 004 sit beside it.

## Cause (established by pm-004; the prime's own cut confirms it)
- On the BYOK Anthropic route (proxied seats), Copilot 1.0.114 uses its **client-side generic tool search**.
  - Feature flag `copilot_cli_tool_search_client_generic: true`.
  - The session event `tool_search.activated` is documented as "generic client-side tool activations".
- Deferred tools are **left out of the tools array** until they are activated, and dropped again afterwards. `copilot_cli_tool_deferral_sticky` is null.
- On CAPI (plain seats), hosted Anthropic tool search keeps the array fixed: plain seats show `availableToolCount` steady at 51, while proxied seats swing between 30 and 50.
- What triggers a change:
  - a `bash` call adds 5 tools (the async-shell companions);
  - `extensions_reload` adds 1, or takes 5 away;
  - the next user turn changes it again.
- Tools lead the cached prefix, so every change re-writes the whole prompt at the 1-hour price. On hot requests that changed the tool list, 78–89% re-wrote more than half the prompt. On hot requests with the same tools, 0–2% did.

## Do, in order
1. **Canary (no machine-wide change).**
   - Make a scratch git repo, for example `/tmp/s07-canary`.
   - Write `.github/copilot/settings.local.json` with `{"toolSearch": false}`. Check the exact key and value type against `copilot help config`; the help text says "`toolSearch`: Enable tool search with deferred tool loading."
   - Spawn **one proxied** Copilot seat there:
     `pij-rs spawn --harness copilot --model claude-opus-5.5 --cwd /tmp/s07-canary --role worker --name s07-canary --json`.
     The receipt must say proxied.
   - Spawn a control seat the same way in a second scratch repo **without** the setting.
   - Drive both through the same script of turns that triggered flips: several `bash` calls, an async/background shell with `read_bash`, an `extensions_reload` if reachable, and user turns in between.
   - From each seat's `~/.copilot/session-state/<id>/events.jsonl` `session.usage_record` entries, record per request: `availableToolCount`, `initiator`, `cacheReadTokens`, `cacheWriteTokens`.
   - **Pass:** the canary's tool count stays flat, with 0 flip re-writes; the control seat reproduces the flips. Also report the prefix size difference (the canary's first-request cache write against the control's).
   - Close both seats afterwards, plainly (they hold no role), and confirm with ps.
2. **STOP and report the canary numbers to me** (pm-004) with the pij_send tool. Do not touch `~/.copilot/settings.json` or any shared repo's settings. The prime takes the canary to Jordan for the machine-wide decision.
3. **sim.py before/after.** Extend `sim.py`, or a copy under your plan dir, so it can re-price a window as if the flip re-writes were gone. Use the canary's measured per-token rates for flat-tool proxied hot turns. Report the fleet result both ways, using egret's method and the same windows. After Jordan's decision, if the setting goes live, re-run it on live seats over a fresh window and compare.
4. **Per-seat TTL, weighed second.**
   - A per-request TTL mix doesn't work: takin showed a 1h marker read over a 5m entry never upgrades it (spike REPORT.md, spike/004-cache-proxy).
   - Evaluate per-seat policies in the sim: for example, a seat uses 1h only when its recent gap history (last N gaps) shows enough 5–60 min gaps to pay for 1h writes, and otherwise 5m.
   - Report each policy's saving next to "1h for all" and "5m for all".
   - Build it in the proxy (`crates/cache-proxy`, seat identity needed per request; propose how) **only if** the sim shows a clear win after the flip fix. If it doesn't pay, say so and stop.

## Where and how
- **Plan:** plan 232 is claimed. Write `docs/plans/232-proxy-tool-flips/` with `/builder`. Its evidence goes in `assets/`.
- **Worktree:** `/Users/jordanknight/pi-hacking/fleet-232-proxy-tool-flips`, branch `coder/232-proxy-tool-flips`, from origin/main. Your seat runs in the main checkout, so work by absolute path and check `git -C <worktree> status` after edits (D-229-1).
- **Builds** (only if step 4 builds): use `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <args>`, niced, `-p` scoped, one at a time.
- **Live seats:** only the two canary seats you spawn. Never touch the shared sidecar's config, its LaunchAgent or the prod daemon. Never type into a pane you didn't create.
- **Cost claims** follow the model-cost rules: price each token class separately, never assume a TTL the transcript doesn't record, and give every number a receipt (command, value, seat ids). takin (model-cost) can check your pricing if you want a second pair of eyes.
- **Docs in the eventual PR:** workstream 004 gains s07, delivering req-01 and req-07 (cost and fidelity). Add a changelog row. Delivers line accordingly.
- **Reviews:** gpt-6.1-sol reviewer, security and performance. Two rounds at most.

Commit trailers: `Pij-Seat: <your seat>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 232-proxy-tool-flips`, plus Co-Authored-By. Stage by path only.

Never:
- change the user-wide `~/.copilot/settings.json`;
- bounce or deploy the prod daemon or sidecar;
- read credential files or print a token;
- `git add -A`, `--no-verify`, or `npm install`.
