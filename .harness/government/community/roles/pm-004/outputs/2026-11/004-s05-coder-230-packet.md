# Packet: coder, workstream 004 s05 (plan 230): F204, the false cache-proxy outage

From pm-004 (pij-outstanding-silkworm), 2026-11-18. You hold `coder/230`. Load the pij skill and the builder skill. Read:
- F204 in the pm-001 LOG: `/Users/jordanknight/pi-hacking/pij-governance/.harness/government/community/roles/pm-001/outputs/LOG.md`, line 1572.
- Blueprint 003 on main: `docs/project/blueprints/003-copilot-cache-proxy.dd.md`, components c-readiness, c-status-route, c-spawn-gate and c-proxy-watch.
- PRD 004 req-09 ("Told once, when it matters").
- Matrix rows 7 and 21: `~/pi-hacking/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.

## The defect (live, 14:56 today)
- The daemon's 30 s watch read `/__pij/status` with a 1 s timeout.
- It got "no answer within 1 s", declared the sidecar "not listening", and sent every prime an outage notice naming 43 proxied seats.
- The sidecar was up the whole time: pid 61526 unchanged, ready, 0 errors.

## Root cause (pm-004, from the source on main)
1. **The status route blocks on upstream.**
   - `crates/cache-proxy/src/proxy.rs`: `status()` (~line 515) awaits `run_check(false)`.
   - When the readiness cache has expired, `run_check` does the `copilot_internal/user` and `/models` GETs (15 s timeout each), serialised behind `check_lock`.
   - The cache expires 10 min after a success, 60 s after a failure, and on every `recheck` after a 402 or a refresh.
   - So about every 10 minutes one status read takes as long as two GitHub round trips.
2. **The daemon treats one slow read as an outage.**
   - `crates/daemon/src/cache_proxy.rs`: `read()` maps a timeout to `Reading::Unreachable("no answer within 1 s")`.
   - `transition()` turns the first unreachable reading into the outage notice.
   - The same 1 s read also gates **spawns**, so a seat spawned during a slow check silently runs plain.

## Fix (both halves, one PR)
**Sidecar (`crates/cache-proxy`):**
- `GET /__pij/status` never waits on upstream. It answers at once from the cached readiness.
- When the cache is stale, or a recheck is due, it starts **one** background check (single-flight; reuse `check_lock`, via spawn or try-lock) and returns the current cached state.
- Startup and `POST /__pij/reload` may still wait for their check; the reload contract says it returns the fresh status.
- The F002 latch and the F001 install rule must still hold. Keep their regressions green.
- Add `checked_at_ms` (epoch ms of the last completed check) to the status JSON as an **additive** field, and update blueprint c-status-route.

**Daemon (`crates/daemon/src/cache_proxy.rs`):**
- A connection **refused** is a real "not listening" signal: a notice at once, as today.
- A **timeout** or unreadable answer counts as "slow". Declare the sidecar down only after **2 consecutive** failed reads, which means 30 s apart on the watch.
- Expose `slow` separately in `/health.cache_proxy`, with a count and the last time. Doctor shows it.
- No notice for a single slow read (row 21's no-false-flap rule). Recovery after a declared outage gives one notice, as today.
- Spawn gate: on a timeout, retry the read **once**, still bounded, before gating plain. The receipt still says why if it ends plain.
- The first-read rule (pm-004 ruling) still applies.

## Tests (each must fail on main and pass after)
- **Sidecar:** with a fake upstream that stalls `copilot_internal/user` for 5 s, a status read after the cache expires returns within 100 ms with the cached state, and exactly one background check runs. Two concurrent status reads start one check, not two.
- **Daemon watch:**
  - one timeout gives 0 notices;
  - two timeouts in a row give 1 notice;
  - a refusal gives 1 notice at once;
  - timeout, ready, timeout gives 0 notices.
- **Gate:** a timeout followed by a successful read gives a proxied seat.
- Mutation-check each guard, and keep the receipts in the plan's evidence.

## Where and how
- **Worktree:** create `/Users/jordanknight/pi-hacking/fleet-230-cache-proxy-watch` with branch `coder/230-cache-proxy-watch` from origin/main.
  - Your seat runs in the main checkout, so work by absolute path.
  - Relative-path edits have hit the main checkout four times (D-229-1). Check `git -C <worktree> status` after every edit batch.
- **Plan:** plan 230 is claimed. Write `docs/plans/230-cache-proxy-watch/` with `/builder`.
- **Builds:** `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/target-ws004 just shared-build -- <args>`, -p scoped, niced, one at a time. Copy your binary out straight after building (D-229-2).
- **Proof:**
  - Use a private daemon and a sidecar on a test port. **Never** the prod daemon, port 7471 or the LaunchAgent.
  - Show live: a forced slow upstream (fake) gives no notice and stays proxied.
- **Cost:**
  - No new timer, launch or per-seat work.
  - The background check replaces the inline one.
  - Measure spawns with `pij-rs --json ping` on your private daemon.
- **Docs in the PR:**
  - blueprint 003 c-status-route, c-readiness and c-proxy-watch updated;
  - workstream 004 gains s05, `delivers` req-09, with a changelog row.
- **Gates:** scoped rust-check (through shared-build), `just lint` from the main checkout, local-path-check, lockfile-allowlist, ddocs-check, and `harness checks --quick`.
- **PR:** `Delivers:` workstream 004 s05 and req-09. Send me the number, head, checks and spawn readings with the pij_send tool.
- Reviews are gpt-6.1-sol seats (reviewer, security, performance), two rounds at most. After the PR is out, push only when a verdict asks, and tell me first.

Commit trailers: `Pij-Seat: <your seat>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 230-cache-proxy-watch`, plus Co-Authored-By. Stage by path only.

Never: bounce or deploy the prod daemon; touch 7471 or the LaunchAgent; type into any pane you didn't create; read credential files or print a token; `git add -A`; `--no-verify`; `npm install`.
