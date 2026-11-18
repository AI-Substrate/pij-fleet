# Plan 229 proof receipts

Rebased on main `d3919152` (s02, #239, merged). Private daemon built from this branch, copied to a private path right after its build (D-229-2), state dir `/tmp/newt229/state2`, port 17561, private tmux server `-L newt229`, `PIJ_CACHE_PROXY_URL=http://127.0.0.1:17571`. Sidecar: the same binary's `pij-rs cache-proxy --port 17571 --state-dir /tmp/newt229/proxy-state` (s02's real sidecar on a test port, its own config file; 7471 and the LaunchAgent untouched), account `jorkni_microsoft`. Live-fleet receipts are s04's.

Command shape: `pij-rs --json spawn --harness copilot --session smoke --cwd /tmp/newt229 --no-wait --model <m> [--no-cache-proxy]`; env from `tmux -L newt229 display -p -t <pane> '#{pane_start_command}'`; notices from `pij-rs --json state <prime> | .data.pendingFyis` and the private store's `fyis` table; sidecar lines from its own log. Prime: `pij-selective-kingfisher` (`orchestration prime set`).

| Row | Setup | Observed | Seats |
|---|---|---|---|
| 1 | no config file | plain, `proxy no_account: no account in /tmp/newt229/proxy-state/cache-proxy.toml`, fix `pij-rs cache-proxy account <login>`; no provider env | pij-increasing-buzzard |
| 3 | ready, `gpt-6.1-sol` | plain, `model gpt-6.1-sol is not Claude`; no provider env | pij-lovely-owl |
| 4 | `cache-proxy account pij-no-such-login-229` | sidecar `no_token`; plain, `proxy no_token: gh has no token for pij-no-such-login-229: …`, fix `gh auth login (as pij-no-such-login-229), then pij-rs cache-proxy on` | pij-mass-whitefish |
| 5 | quota exhausted | not reproducible on this account (quota usable); proven against a stub serving `quota_exhausted`: plain, `proxy quota_exhausted: quota exhausted until 2026-12-01` (`decide_is_proxied_only_when_ready_and_the_base_id_is_served`, first stub run) | — |
| 6 | sidecar stopped | plain, `proxy not listening`, fix names `just cache-proxy-autostart` | pij-fascinating-angelfish |
| 7 | sidecar killed under 4 live proxied seats | 1 FYI to the prime naming all four and the fix; next tick 0 more (delta 1 → 1). A turn on pij-colorful-llama then retried and failed: `Could not connect to local model provider at http://127.0.0.1:17571`. Sidecar restarted: 1 FYI `ready (was not_listening)`. Copilot's own `/restart` is s04's | pij-colorful-llama, pij-complex-snail, pij-formal-tiger, pij-mass-echidna |
| 8 | daemon bounced at 03:28:18 during two proxied turns that began streaming at 03:28:14 | both turns finished (1 to 2500, no error, no retry); sidecar pid 8150 before and after; after the bounce `/health.cache_proxy` `ready`, counters `requests 6`, `notices 0` | pij-colorful-llama, pij-complex-snail; daemon pids 8149 → 20604 → 24363 |
| 12 | account switched pij-no-such-login-229 → jorkni_microsoft | next spawns proxied; sidecar `upstream for jorkni_microsoft: api.enterprise.githubcopilot.com`; doctor names the account | pij-colorful-llama |
| 13 | `--no-cache-proxy`; `cache-proxy off` | plain `opted out with --no-cache-proxy`; plain `proxy off: switched off (pij-rs cache-proxy off)`, fix `pij-rs cache-proxy on`; no provider env | pij-constant-toucan, pij-genuine-earwig |
| 18 | a Copilot seat's `cache_proxy` set NULL in the store | `/health.cache_proxy.seats.plain` and the doctor `cache seats:` line list it | pij-lovely-owl |
| 19 | single human account | `cache-proxy status` prints no warning; the two-account warning is s02's parser test | — |
| 20 | `--harness omp` on a Claude model | no `cache_proxy` field, no provider env | pij-imaginative-nightingale |
| 21 | `cache-proxy off`, then `on` | exactly 2 FYIs to the prime ("off … new Copilot spawns run plain … unaffected", "ready (was off)"); next tick 0 | pij-selective-kingfisher |
| 22 | `claude-opus-5.5` and `claude-opus-5.5-1m` | both proxied with `COPILOT_PROVIDER_TYPE=anthropic`, `BASE_URL=http://127.0.0.1:17571`, `MODEL_ID=claude-opus-5.5`, `MAX_PROMPT_TOKENS=1000000`, `VISION=true`; both status bars `0/1.0M`; one turn each: sidecar `status=200`, `model=claude-opus-5.5` and `model=claude-opus-5.5 (from claude-opus-5.5-1m)`, `1h=32591` and `1h=12266`, `5m=0`. `claude-sonnet-5.5` and `claude-haiku-5.5` proxied with `MAX_PROMPT_TOKENS=936000` | pij-colorful-llama, pij-complex-snail, pij-formal-tiger, pij-mass-echidna |

Seen on Copilot 1.0.114 with the provider env, recorded for s04: the `-1m` seat shows `Model "claude-opus-5.5-1m" from --model flag is not available. Using "claude-opus-5.5" instead.` yet still sends `claude-opus-5.5-1m` (the sidecar maps it); every proxied seat shows `Provider type "anthropic" typically requires authentication` and works without a key.

## Cost (k-4, k-5)

`pij-rs --json ping | jq .data.spawns`, daemon pid 24363, sidecar ready: before two Copilot Claude spawns `total 24`; after `total 28` (`tmux.new-window` +2, `tmux.pipe-pane attach` +1, `tmux.set-options tap owner` +1: the existing per-spawn launches); 62 s and two watch ticks later `total 30` (`tmux.pipe-pane attach` +1, `tmux.set-options tap owner` +1, the second pane's tap). No op is the cache proxy's: 0 launches per spawn gate, 0 per watch tick. The sidecar's own `gh launches` stayed at 2 for the whole run (1 per account set).

## Doctor rows

With the private binary first on `PATH`, `PIJ_CACHE_PROXY_PORT=17571` and the private daemon's address: `cache-proxy: ready on 127.0.0.1:17571 … account: jorkni_microsoft … upstream: api.enterprise.githubcopilot.com …` (s02's `status`), then `cache seats: proxied: pij-colorful-llama, pij-complex-snail, pij-formal-tiger, pij-mass-echidna; plain: pij-constant-toucan, pij-fascinating-angelfish, pij-genuine-earwig, pij-increasing-buzzard, pij-lovely-owl, pij-mass-whitefish, pij-selective-kingfisher`.

## Mutation receipts

Each guard mutated on the review-round head, its test run through `just shared-build -- test -p pij-daemon --lib <filter>`, the source restored after: every mutant failed its test (exit 101, `1 failed`).

| Mutant | Change | Test that failed |
|---|---|---|
| M1 rollover copy | `if !same_launch {` → `if false {` (registration) | `native_sqlite_http_new_session_supersedes_seat_without_stranding_new_work` |
| M2 first-read rule | `None if state == "ready" \|\| proxied.is_empty() => return None` → `None => return None` | `first_read_reports_only_a_problem_under_proxied_seats` |
| M3 prime dedup | `primes.dedup();` removed | `a_tick_tells_each_live_prime_once_and_then_nothing` |
| M4 ready-state check | `!status.ready \|\| status.state != "ready"` → `!status.ready` | `decide_needs_both_ready_and_the_ready_state` |
| M5 off wording | the `off` arm never matches | `off_is_told_as_new_spawns_plain_existing_seats_unaffected` |
| M6 no redirects | `.redirect(Policy::none())` removed | `a_redirecting_status_route_is_not_followed` |
