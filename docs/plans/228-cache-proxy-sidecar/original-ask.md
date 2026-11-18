# Original ask

Packet from pm-004 (pij-outstanding-silkworm), 2026-11-18, copied verbatim, then the same-day contract change (req-13).

## Packet: coder, workstream 004 s02 (plan 228: the cache-proxy sidecar)

From pm-004 (pij-outstanding-silkworm), 2026-11-18. You hold `coder/228` for workstream 004 s02. Load the pij skill and the builder skill. Read these in this order:
1. Blueprint 003 (`<pi-hacking>/ws004-docs/docs/project/blueprints/003-copilot-cache-proxy.dd.md`; PR #238, not merged yet).
2. PRD 004 (same folder, `prds/004-copilot-cache-proxy.dd.md`).
3. Workstream 004, step s02.
4. The acceptance matrix: `<pi-hacking>/pij-governance/.harness/government/handover/cache-proxy-test-matrix.md`.
5. The rulings: `handover/ruling-2026-11-18-copilot-cache-proxy.md` in the same folder.

## What you build
The sidecar, blueprint components c-proxy-core, c-token-holder, c-readiness, c-status-route, c-proxy-verb, c-config-file and c-launch-agent:
- A new pi-free crate, `crates/cache-proxy`, in the workspace.
  - Server: axum + hyper-util, the way `crates/daemon/src/serve.rs` does it.
  - Upstream: reqwest, streaming.
- `pij-rs cache-proxy`: a new `Command::CacheProxy` in `crates/cli/src/main.rs`.
  - With no action it runs the sidecar; wire it as an early long-running branch beside `Command::Daemon`.
  - Actions: `status`, `account <login>`, `off`, `on`.
- `~/.pij-rs/cache-proxy.toml` (`account`, `enabled`). No file or no account means off.
- justfile recipes `cache-proxy-autostart` / `cache-proxy-autostart-off`.
  - Same shape as `rs-autostart`, plus KeepAlive and ThrottleInterval 10.
  - `just install` calls `cache-proxy-autostart`.
- The second-human-account check inside `status`.
  - A pure parser of `/usr/bin/dscl . -list /Users UniqueID`, with a unit test (matrix row 19). s03 calls it from doctor.

## Start from the spike
- The POC is on local branch `spike/004-cache-proxy` (9a397862) in `<pi-hacking>/ws004-spike`, at `spikes/copilot-ttl-proxy/src/main.rs`. Report: `spikes/REPORT.md`.
- Port its rewrite (`set_ttl`, `initiator`), its token holder (401 → single-flight refresh, one retry) and its quota gate. Rename the env knobs to `PIJ_CACHE_PROXY_*`.

## Fixed facts (don't relitigate)
- Token: `gh auth token --user <account>`.
  - `gh` is resolved once to an absolute path and recorded through `pij_core::spawns::record("gh.cache_proxy_token")`. `crates/testkit/tests/spawn_sites.rs` must pass.
  - The token lives in memory only: never logged, never written, never in argv.
  - Refresh only on a 401. There is no proactive refresh (the `gho_` token has no expiry).
- Upstream: `copilot_internal/user` → `endpoints.api`.
- Quota: `quota_snapshots.premium_interactions` (usable if `unlimited`, `remaining > 0` or `overage_permitted`).
  - Cached for 10 minutes per account, and re-checked after a refresh or a 402.
  - A 402 marks the quota exhausted at once.
- Bind 127.0.0.1:7471 only.
- Logs carry host, status and cache-usage counts, never a header value.
- **The status route contract is shared with s03, which builds against it in parallel. Match it exactly:** blueprint c-status-route.
  - `GET /__pij/status` returns `{ready, state, reason, fix, account, upstream_host, quota: {usable, reset}, counters: {requests, refreshes, gh_launches, upstream_401, upstream_402}, pid, since}`.
  - `state` is one of `ready`, `off`, `no_account`, `gh_missing`, `no_token`, `quota_exhausted`, `upstream_unreachable`.
  - `POST /__pij/reload` takes no body.
  - If you must change the contract, ask me first, and I'll tell s03.

## Where and how
- Worktree: `<pi-hacking>/fleet-228-cache-proxy-sidecar`, branch `coder/228-cache-proxy-sidecar` (from origin/main 84ef0774).
  - Your seat runs in the main checkout (OMP can't start in a linked worktree), so work there by absolute path.
- Plan: claim is done (plan 228). Write `docs/plans/228-cache-proxy-sidecar/` with `/builder`: plan, then tasks, then implement.
- **Builds go through `just shared-build` only, niced, one at a time:**
  - `PIJ_SHARED_TARGET=<pi-hacking>/target-ws004 just shared-build -- <cargo args>`.
  - The s03 coder shares this target, and the lock serialises you.
  - One build per change; use -p scoping.
  - If shared-build refuses on the disk floor, stop and tell me. Never work around it.
- Proof before the PR: matrix rows 2, 9, 10, 11, 14, 15, 16 and 17.
  - Use **plain `copilot` processes** you start yourself, with `COPILOT_PROVIDER_TYPE=anthropic` and `COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:<test port>`, in a tmux session you own and name `ws004-s02-test`.
  - Use a test port, not 7471, unless no other proxy is running.
  - Account: `jorkni_microsoft`. `jakkaj` is out of quota; use it only to show the quota refusal.
  - Record each row as: command, observed value.
  - Do not install the LaunchAgent on this machine. Prove its plist with `plutil -lint` on a temp path.
- Gates before the PR: scoped `just rust-check` (through shared-build), `just lint` from the main checkout, `just local-path-check`, `just lockfile-allowlist`, `just ddocs-check`. Then `harness checks --quick`.
- Open the PR against main.
  - `Delivers:` workstream 004 s02; req-01, req-02, req-03, req-06, req-07, req-10.
  - Put the row receipts in the body.
  - Send me the number, head, checks and the spawn-counter readings. Reviews are Copilot gpt-6.1-sol seats (reviewer, security, performance), two rounds at most.
- After the PR is out for verdicts, push nothing unless a verdict asks, and tell me first. Never use a question UI; ask me in plain text.

Commit trailers: `Pij-Seat: <your seat id>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 228-cache-proxy-sidecar`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit. Reply with `pij send pij-outstanding-silkworm --body-file - <<'PIJ'` … `PIJ`.

Never:
- restart, bounce or deploy the prod daemon;
- install either LaunchAgent;
- type into any pane you didn't create;
- read credential files or print a token;
- `git add -A`, `--no-verify`, or push `refs/notes/*`;
- `npm install`.

## Contract change (pm-004, 2026-11-18; blueprint 003 updated at ws004-docs a8fa9c5b)

1. Proxy core: map any `*-1m` model to its base id before forwarding. Copilot's API returns 400 model_not_supported for `claude-opus-5.5-1m`; the base id serves 1M. Log it as `model=<base> (from <x-1m>)`. The POC already does this: spike/004-cache-proxy 6566e91f.
2. Readiness: also GET `<endpoints.api>/models`, cached with the user check (10 min per account, re-run after a refresh or a 402).
3. The status route gains `models: {"<base id>": {max_prompt_tokens}}`, Claude ids only, keyed by base id, never `-1m`.
4. Your proof adds matrix row 22: one plain copilot process on each name, both get 200s, a 1.0M window, and only 1h writes.
