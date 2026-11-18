# Plan 229 proof receipts (interim: stub sidecar)

Private daemon `pij-rs` built from `coder/229-cache-proxy-gate` (copied to a private path, because the shared target is overwritten by the s02 lane's builds), state dir `/tmp/newt229/state`, port 17561, private tmux server `-L newt229`, `PIJ_CACHE_PROXY_URL=http://127.0.0.1:17571`. The sidecar is a stub serving the c-status-route body exactly as s02 types it (crane via pm-004, 2026-11-18). These rows are re-run against s02's real sidecar after the rebase; live-fleet receipts are s04's.

Command shape: `pij-rs --json spawn --harness copilot --session smoke --cwd /tmp/newt229 --no-wait --model <m> [--no-cache-proxy]`; env read from `tmux -L newt229 display -p -t <pane> '#{pane_start_command}'`; notices read from the private store's `fyis` table and `pij-rs --json state <prime> | .data.pendingFyis`. Prime: `pij-contemporary-tahr` (designated with `orchestration prime set`).

| Row | Setup | Observed | Seats |
|---|---|---|---|
| 1 | stub `no_account` | plain, `proxy no_account: no account is set`, fix `pij-rs cache-proxy account <login>`; no provider env | pij-inner-caribou |
| 3 | stub ready, `gpt-6.1-sol` | plain, `model gpt-6.1-sol is not Claude`; no provider env | pij-organisational-skink |
| 5 | stub `quota_exhausted` | plain, `proxy quota_exhausted: quota exhausted until 2026-12-01` | pij-hissing-gopher |
| 6 | stub stopped | plain, `proxy not listening`, fix names the sidecar start | pij-alternative-chinchilla |
| 7 | stub stopped under 2 proxied seats | 1 FYI to the prime: "not listening … Proxied seats' model calls fail: pij-evident-opossum, pij-formal-elk. Fix: …"; next tick 0 more (pendingFyis 1 → 1) | pij-evident-opossum, pij-formal-elk, prime pij-contemporary-tahr |
| 8 | daemon restarted, stub ready | `notices 0`, pendingFyis unchanged (3 → 3); restarted with stub down under proxied seats: 1 notice (pm-004 ruling) | daemon pids 85129, 9890, 17806 |
| 12 | stub ready, `account` → octo-b | spawn proxied; `/health.cache_proxy.status.account` = octo-b, upstream `api.business.githubcopilot.com` | pij-total-jackal |
| 13 | `--no-cache-proxy`; stub `off` | plain `opted out with --no-cache-proxy`; plain `proxy off: …` fix `pij-rs cache-proxy on`; no provider env | pij-cloudy-echidna, pij-specific-swift |
| 18 | a Copilot seat's `cache_proxy` set NULL in the store | `/health.cache_proxy.seats.plain` lists it | pij-organisational-skink |
| 20 | `--harness omp` on a Claude model | no `cache_proxy` field, no provider env, no status read | pij-overall-hornet |
| 21 | stub ready → off → ready | exactly 2 FYIs to the prime ("off … unaffected", "ready (was off)"); next tick 0 | pij-contemporary-tahr |
| 22 | stub ready, `claude-opus-5.5` and `claude-opus-5.5-1m` | both proxied; env `COPILOT_PROVIDER_TYPE=anthropic`, `BASE_URL=http://127.0.0.1:17571`, `MODEL_ID=claude-opus-5.5`, `MAX_PROMPT_TOKENS=936000`, `VISION=true`; `claude-sonnet-9` plain `model not served by Copilot API` | pij-recent-octopus, pij-low-cicada, pij-applicable-nightingale |

## Cost (k-4, k-5)

`pij-rs --json ping | jq .data.spawns`, daemon pid 65297: before 3 Copilot Claude spawns `total 26`; after `total 33` (`tmux.new-window` +3, `tmux.pipe-pane attach` +2, `tmux.set-options tap owner` +2: the existing per-spawn launches); 64 s and two watch ticks later `total 37` (`tmux.pipe-pane attach` +1, `tmux.set-options tap owner` +1 for the third pane's tap, `git.pa-watchdog-common-dir` +2 from the existing PA watchdog). No op is the cache proxy's: 0 launches per spawn gate, 0 per watch tick.

## Doctor rows

The two new `just doctor` lines, run with the private binary first on `PATH` and the private daemon's address: `cache-proxy: (unknown: pij-rs predates the cache proxy — run: just rs-install)` (this branch has no `cache-proxy` verb until it rebases on s02) and `cache seats: proxied: pij-evident-opossum, pij-formal-elk, pij-low-cicada, pij-rapid-shark, pij-recent-octopus, pij-total-jackal; plain: pij-alternative-chinchilla, pij-applicable-nightingale, pij-cloudy-echidna, pij-contemporary-tahr, pij-hissing-gopher, pij-inner-caribou, pij-organisational-skink, pij-specific-swift`.

## Not yet proven

- Every row against s02's real sidecar (after the rebase), including the `cache-proxy:` doctor line with the second-human-account warning (row 19 is s02's parser test).
- Row 7's `/restart`: Copilot's own restart re-execs in place [INFERENCE: it keeps the provider env]; the notice's fix names `pij-rs revive`, which re-runs the gate (`revive_keeps_an_opt_out_and_re_runs_the_gate_otherwise`).
