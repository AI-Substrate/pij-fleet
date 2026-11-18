# The Copilot cache proxy (`pij-rs cache-proxy`)

A sidecar that gives Copilot seats on Claude models a 1-hour prompt cache, so a pij
message that wakes a seat idle for more than 5 minutes reads its context from cache
instead of rebuilding it. PRD 004, blueprint 003, plan 228 (this sidecar); the daemon's
spawn gate is plan 229.

## What it is

- **Same binary, own process.** `pij-rs cache-proxy` listens on `127.0.0.1:7471` only,
  under its own LaunchAgent, so a daemon bounce never cuts a proxied seat's stream.
- **One account, token in memory.** It runs `gh auth token --user <account>` (gh found
  once at start: `PIJ_GH`, else PATH) and keeps the token in memory only. It is never
  logged, written to disk or passed in an argument list.
- **The rewrite.** On `POST /v1/messages` it adds `ttl: "1h"` to every ephemeral
  `cache_control` without one, sends `X-Initiator: agent` when the last message only
  returns tool results (so a tool continuation isn't billed as a user request), and
  forwards a `*-1m` model as its base id (Copilot refuses `-1m`; the base id serves 1M).
  Everything else, including the streamed response, passes through unchanged.
- **The endpoint comes from the token.** `copilot_internal/user` names `endpoints.api`,
  so individual, business and managed enterprise accounts need no setting.

## Use it

```bash
just cache-proxy-autostart                      # install and start the LaunchAgent (just install does this)
pij-rs cache-proxy account <gh login>           # name the account; reloads the sidecar
pij-rs cache-proxy status                       # state, account, upstream, quota, models, counters
pij-rs cache-proxy off                          # new spawns run plain; live proxied seats keep working
pij-rs cache-proxy on                           # back on; re-runs the checks (and refetches a failed token)
just cache-proxy-autostart-off                  # remove the LaunchAgent (stops the sidecar)
```

`account`, `off` and `on` write `~/.pij-rs/cache-proxy.toml` (mode 0600; `account`,
`enabled`), then `POST /__pij/reload`. No file, or no account, means not ready. The
sidecar logs to `~/.pij-rs/cache-proxy.log`: start, state changes, and per request the
upstream host, model, initiator, status and cache-usage counts. Never a header value.

`status` also warns when the Mac has a second human account (`/usr/bin/dscl`), because
the loopback port is reachable from every account. `--json` gives the envelope:
`data.{port, listening, status, fix, config_written, accounts_warning, accounts_error}`.

## The status route (the contract the daemon reads)

`GET /__pij/status` returns:

```json
{"ready": true, "state": "ready", "reason": "", "fix": "",
 "account": "octo_corp", "upstream_host": "api.enterprise.githubcopilot.com",
 "quota": {"usable": true, "reset": "2026-12-01"},
 "models": {"claude-opus-5.5": {"max_prompt_tokens": 1000000}},
 "counters": {"requests": 0, "refreshes": 0, "gh_launches": 1, "upstream_401": 0, "upstream_402": 0},
 "pid": 74027, "since": 1794968379799}
```

| state | means | fix |
|---|---|---|
| `ready` | token held, endpoint known, quota usable, switched on | |
| `off` | `pij-rs cache-proxy off` | `pij-rs cache-proxy on` |
| `no_account` | no config file or no account (or the file is unreadable) | `pij-rs cache-proxy account <login>` |
| `gh_missing` | no gh at start | install gh or set `PIJ_GH`, restart the sidecar |
| `no_token` | gh gave no token, GitHub refused it (401), or a refresh failed | `gh auth login`, then `pij-rs cache-proxy on` |
| `quota_exhausted` | premium quota used up (reset date in `quota.reset`), a 402 from upstream, or no Copilot entitlement | wait for the reset, or another account |
| `upstream_unreachable` | `copilot_internal/user` or `/models` unreachable | check the network; `on` re-runs the check |

`since` is epoch milliseconds of the last state change; `models` lists Claude ids only,
never `-1m`. `POST /__pij/reload` takes no body. Neither route carries a token.

## Token refresh and readiness

- The `gho_` token has no expiry, so there is no timed refresh. A 401 triggers one
  refresh shared by every waiting request, and each request retries once with the
  refreshed token.
- A failed refresh keeps the current token, moves the state to `no_token` once, and
  launches nothing more until a reload (`on`, `account`), so it can never loop. A 401
  with no newer token to try is returned to the seat, not retried.
- Readiness (`copilot_internal/user` plus `<endpoints.api>/models`) is cached per account
  for 10 minutes (60 s after a failed check) and re-run on the next status read once it
  ages out, after a refresh, and after a 402. A 402 marks the quota exhausted at once.

## Safety

- Binds `127.0.0.1` only. Any request with an `Origin` header, or a `Host` other than
  `127.0.0.1:<port>` or `localhost:<port>`, gets 403 before anything is forwarded, so a
  web page or a rebound DNS name can't spend the account.
- With no token held, requests get 503 with the state and its fix.

## Cost

No launch per request. One `gh` per sidecar start (and per account change, or `on` after
a failed fetch), at most one per 401 burst, none after a failed refresh until a reload.
One `dscl` per `status` run. No timer or poll: at most two HTTPS GETs per 10 minutes per
account while the status is being asked. Both launch sites are counted
(`gh.cache_proxy_token`, `dscl.cache_proxy_accounts`) and scanned by
`crates/testkit/tests/spawn_sites.rs`.

## Knobs

| variable | use |
|---|---|
| `PIJ_CACHE_PROXY_PORT` (or `--port`) | another loopback port, for tests |
| `PIJ_GH` | gh's absolute path, read once at start |
| `PIJ_RS_STATE_DIR` (or `--state-dir`) | where `cache-proxy.toml` lives |
| `PIJ_CACHE_PROXY_TEST_STALE_TOKEN=1` | test only: hold a stale token after the start-up check, so the next request takes the 401 path |
| `PIJ_CACHE_PROXY_GITHUB_API` | test only: a fake GitHub API base (allows a plain-http endpoint) |

To point a plain `copilot` at it by hand:
`COPILOT_PROVIDER_TYPE=anthropic COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:7471
COPILOT_PROVIDER_MODEL_ID=claude-opus-5.5 COPILOT_PROVIDER_MAX_PROMPT_TOKENS=1000000 copilot`.
