# Plan 228 live receipts (matrix rows 2, 9, 10, 11, 14, 15, 16, 17, 22; AC10)

Plain `copilot` 1.0.114 processes in tmux session `ws004-s02-test` (own socket `-L ws004-s02`), each with
`COPILOT_PROVIDER_TYPE=anthropic COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:<port>
COPILOT_PROVIDER_MODEL_ID=<id> COPILOT_PROVIDER_MAX_PROMPT_TOKENS=1000000`, against the branch's debug `pij-rs`
(copied to `/tmp/ws004-s02/pij-rs`) running `pij-rs cache-proxy --port 7481` (account `jorkni_microsoft`,
`PIJ_RS_STATE_DIR=/tmp/ws004-s02/state`) and `--port 7482` (stale-token knob, `state9`). No pij seat was used, 7471
was never bound, no LaunchAgent was installed. 2026-11-18, UTC times from the sidecar log.

| row | command | observed |
|---|---|---|
| setup | `pij-rs cache-proxy --port 7481` with no config, then `status` | `no_account`, fix `pij-rs cache-proxy account <login>`, gh launches 0 |
| setup | `pij-rs cache-proxy --port 7481 account jorkni_microsoft` | `ready`, upstream `api.enterprise.githubcopilot.com`, models claude-haiku-4.5 136000, claude-haiku-5.5 936000, claude-opus-5.5 1000000, claude-sonnet-5 936000, claude-sonnet-5.5 936000; gh launches 1; config `-rw-------` holding only `account` and `enabled` |
| 2 | window `a`: `copilot --model claude-opus-5.5` (MODEL_ID claude-opus-5.5); turn A 02:20:14, idle 7 m 31 s (sidecar restarted on the final binary meanwhile), turn B 02:27:45 | A: `cache_read=0 cache_write=35832 1h=35832 5m=0`; B: `cache_read=35832 cache_write=50 1h=50 5m=0`, status 200 |
| 22 | window `b`: `copilot --model claude-opus-5.5-1m` (MODEL_ID claude-opus-5.5-1m), same two turns | log `model=claude-opus-5.5 (from claude-opus-5.5-1m) ... status=200` both turns; A `1h=13740 5m=0`, B `cache_read=36202 cache_write=50 1h=50 5m=0`; status bars of `a` and `b` show `…/1.0M` |
| 9 | 3 concurrent `copilot -p` on port 7482, `PIJ_CACHE_PROXY_TEST_STALE_TOKEN=1` (final binary, `stale-row9b.log`) | 3× `status=401: retrying once with the refreshed token`, 1× `token refreshed after a 401 (gh launches 2)`, 3× `status=200`, all 3 clients answered `ok` (exit 0); counters `requests 3, refreshes 1, gh_launches 2, upstream_401 3`; one state line only (`-> ready`), no flap |
| 10 | 1 `copilot -p` on a fresh stale-token sidecar | `status=401` → `token refreshed after a 401 (gh launches 2)` → `status=200`, turn answered `ok`; counters `requests 1, refreshes 1, gh_launches 2, upstream_401 1` |
| 11 | stale-token sidecar with `PIJ_GH=/tmp/ws004-s02/gh-once` (real gh on its first run, fails after; stands in for `gh auth logout` without logging the shared account out), 3 `copilot -p` turns (Copilot retries each 3×) | gh-once launched twice (start 02:33:31, refresh 02:33:41); one `refresh failed, keeping the current token; no further refresh until a reload`; one `state ready -> no_token`; 9 requests → 9× `401: no newer token (refresh failed or latched until a reload); not retried`; counters `requests 9, refreshes 1, gh_launches 2, upstream_401 9`; no loop. `gh: /tmp/ws004-s02/gh-once` logged once at start: `PIJ_GH` is read only there |
| 14 | `account jakkaj`, then `account jorkni_microsoft` | both accounts' `endpoints.api` is `api.enterprise.githubcopilot.com` (both report `copilot_plan: enterprise`), shown host-only in the log and status. No individual or business account is available on this machine |
| 5 (refusal) | `account jakkaj` | `quota_exhausted`, reason `premium quota exhausted for jakkaj until 2026-12-01`; switching back gives `ready` |
| 15 | `copilot -p` asked for 5 sequential shell calls | 6 requests: 1 `initiator=user`, 5 `initiator=agent`, all 200. Premium delta not isolatable: the account is shared with live fleet seats and `copilot_internal/user` remaining lags (unchanged across the turn, then −1,249 over 6 minutes fleet-wide) |
| 16 | same prompt (list files, view `icon.png`, sum `numbers.txt`, write and read `sum.txt`) proxied vs plain | plain: `image: gray calculator app icon, orange; sum: 20100`. Proxied without `COPILOT_PROVIDER_VISION`: tools fine, image skipped ("image input isn't available for this model"). Proxied with `COPILOT_PROVIDER_VISION=true`: `image: Black calculator app icon, grey; sum: 20100`, 1 user + 5 agent requests, 200s, `5m=0`. Spawn env needs `COPILOT_PROVIDER_VISION=true` (sent to pm-004 for s03) |
| 17 | 10 concurrent `copilot -p` on 7481 | counters before `requests 0, gh_launches 1`, after `requests 10, gh_launches 1, refreshes 0`; 10/10 exit 0. Daemon untouched: `pij-rs --json ping .data.spawns.total` 475 → 475 over 20 s |
| AC10 | `gh auth token --user <acct> \| grep -c -F -f - <logs, configs, outputs>` for both accounts; `grep -c gho_`; same against `ps -axww -o args` with a proxied copilot live | 0, 0, 0, 0 |
| plist | `just cache-proxy-autostart <tmp>/dev.pij.cache-proxy.plist` | `plutil -lint`: OK; KeepAlive true, ThrottleInterval 10, RunAtLoad true, `/bin/sh -c exec "<npm-prefix>/bin/pij-rs" cache-proxy`, logs `~/.pij-rs/cache-proxy.log`; no agent in `~/Library/LaunchAgents` |
| 19 | `cargo test -p pij-cache-proxy accounts` | two human accounts warn, one does not; live `status` on this Mac printed no warning |
