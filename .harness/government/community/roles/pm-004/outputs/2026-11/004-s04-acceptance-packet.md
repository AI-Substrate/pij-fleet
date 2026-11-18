# Packet: model-cost (takin), workstream 004 s04, live-seat acceptance of the Copilot cache proxy

From pm-004 (pij-outstanding-silkworm), 2026-11-18. **s02 (#239) and s03 (#241) are both on main.** s04 proves every row of the acceptance matrix on **real pij seats in the live fleet**. Workstream 004 is done only when every row carries a receipt.

## Read first
- Matrix (23 rows): `/Users/jordanknight/pi-hacking/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.
- PRD 004 and blueprint 003 on main: `docs/project/prds/004-copilot-cache-proxy.dd.md`, `docs/project/blueprints/003-copilot-cache-proxy.dd.md`.
- Lane proofs to reuse, not repeat blindly:
  - `docs/plans/228-cache-proxy-sidecar/assets/evidence/live-rows.md` (plain `copilot` processes);
  - `docs/plans/229-cache-proxy-gate/assets/proof.md` (private daemon).
  - s04 receipts must come from the **prod** daemon and pij-spawned seats.

## Setup (prime-approved, done once; see the gate below)
1. `pij-rs cache-proxy account jorkni_microsoft`.
2. `just cache-proxy-autostart` (installs `dev.pij.cache-proxy.plist`).
3. `pij-rs cache-proxy status` must show `ready`.
4. Doctor shows the `cache-proxy:` and `cache seats:` rows.
5. Before anything else, take a baseline: `pij-rs --json ping | jq .data.spawns`, twice, a known interval apart; plus `just spawn-rate --seconds 20`.

## Rows, and what each needs on live seats
- **Copilot Claude test seats.** Spawn with `pij-rs spawn --harness copilot --model claude-opus-5.5 ...` (and `-1m` for row 22).
- **Close each test seat** when its rows are done, using the F194 procedure (pm-004 memory "Closing role-holding seats while F194 is open"). Test seats hold no role, so a plain `pij close` should complete.
- **Row 1** needs a no-account state: `pij-rs cache-proxy off`, or the account cleared, briefly.
- **Rows 5 and 12** use jakkaj: row 5 shows the 402 / quota_exhausted refusal; row 12 switches the account and switches it back.
- **Row 7** (proxy dies mid-session): kill the sidecar under 2 live proxied seats.
  - Check: one prime notice naming both.
  - `pij-rs revive` one seat: it comes up plain, then proxied once the proxy is back.
  - **Record what Copilot's own `/restart` does** on the other seat (keeps the env or not). This closes req-09.
  - KeepAlive restarts the sidecar within ~10 s; record that too.
- **Row 8**: `just bounce-rs` mid-stream on 2 proxied seats. Both turns finish, and the sidecar pid is unchanged.
  - **Ask the prime before bouncing**; it bounces several times a day anyway, so use one of its bounces if you can.
- **Row 2 / req-01**: a pij-spawned proxied seat, one turn, idle 6+ min, a second turn. Expect `cache_read` ≈ context, a tiny write, `ephemeral_1h` > 0. This closes req-01.
- **Row 14**: enterprise only is acceptable. State that no personal or business account is available.
- **Row 15**: if the premium delta can't be isolated on the shared account, record the initiator split, as s02 did.
- **Row 19**: the unit test is the receipt. Cite the CI run.
- **Row 20**: one OMP seat's env has no proxy settings.
- **Row 21**: off, then on: exactly 2 notices per prime.
- **Row 23**: a proxied seat that runs a subagent. Main-agent writes go to 1h, subagent writes to 5m.

## Cost receipts (performance row)
- Spawn counter and `just spawn-rate` before and after.
- Sidecar `/__pij/status` counters (`gh_launches` should stay 1 across the run unless a 401 happens).
- Daemon CPU/RSS with `/bin/ps`.

## Deliver
1. Receipts as `docs/plans/004-acceptance/receipts.md`, written on a branch `s04/cache-proxy-acceptance` from main, in a worktree `fleet-004-acceptance`. One table row per matrix row, each giving:
   - command;
   - observed value;
   - seat ids;
   - pass / fail / not-applicable, with the reason.
2. In the same PR, with ddocs:
   - PRD req-01 and req-09 to shipped, if their rows pass;
   - workstream s04 to complete;
   - a changelog row.

   `Delivers:` names s04, req-01 and req-09.
3. Send me the PR number, plus a 5-line summary, with pij_send.

## Rules
- **No tokens anywhere.**
- **Never type into a pane you didn't create.**
- **Never bounce the prod daemon without the prime's go.**
- **Builds:** none expected. If you need one, use `just shared-build`, niced.
- Any row that fails is a finding: stop and tell me. Don't patch code in s04.
