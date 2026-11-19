# Retro: pij-joyous-sailfish, plan 232 (workstream 004 s07, proxied tool-list flips)

**Role:** coder/232, OMP on github-copilot/claude-opus-5.5-1m. **PM:** pm-004 (pij-outstanding-silkworm).

**Delivered:**
- Evidence that `toolSearch: false` stops most proxied tool-list flips, with the residual flips it leaves measured. The setting itself was applied user-wide by the prime on Jordan's go and is not code in this PR.
- A canary seat driven through bash, read_bash and extensions_reload.
- `flip_window.py`, which reports flip rates split by Copilot process start.
- `sim232.py`, which re-prices egret's windows with the flips removed and the larger tool prefix paid for. Every token class is priced at its own record's listed price, and the sum matches billed exactly.
- A per-seat TTL verdict: keep 1h and build nothing. Even a hindsight bound saves at most 0.9-5.4%.

**magicWand:** Copilot's `copilotUsage.tokenDetails` already gives every request's classes and prices, so a cost question reduces to moving tokens between classes. A shared `just fleet-cost` library, holding the loader, the process-start split, flip detection and class pricing, would let the next cost question start from calibrated numbers instead of a third copy of egret's loader.

**difficulties:**
- D-232-1: `toolSearch` is user-scope only, so the packet's per-repo canary was impossible as written. The first step therefore became a report rather than a canary.
- D-232-2: the spawn into a new /tmp repo timed out on Copilot's folder-trust prompt. `pij_close` couldn't reach a child seat whose cwd was outside the project.
- D-232-3: ddocs re-serialised the workstream with spaces, and biome refused it.
- D-232-4: with `NO_COLOR` and `FORCE_COLOR` both set, one TS test fails on Node's stderr warning.
- `availableToolCount` misses a whole flip class: extensions_reload changed `toolTokenCount` with the count unchanged. Detect flips on both fields.
- Round-1 review (antlion, stingray), three findings:
  - The first sim fitted its historical rates on every record loaded, so each later session moved the "historical" numbers, and the PR prose already disagreed with its own output. Bound every historical estimate to its declared window, and make the script fail when a later record changes it.
  - The first "fixed" estimate removed flips the canary had just shown the setting keeps. A counterfactual removes only what the evidence shows the fix removes; anything more is a bound, and it has to be labelled one.
  - The TTL sim didn't clear the cache at a process restart.
- One canary is not the fleet: a resumed process added +5 tools on its first bash call (e69a6a28), which the canary never exercised.
