# Velocity log

One row per phase end: start/end and output. Each successive extension should
be faster than the last.

| Date | Work | Start | End | Output |
|------|------|-------|-----|--------|
| 2026-10-13 | Plan 169 HydraFusion support (pij-gleaming-sheep) | 09:47 | 10:50 | spawn tier/effort fix, R3 live canary (single x4, cascade x2, critique unobserved), cascade inbox-stall fix + fusion activity |
| 2026-10-27 | Plan 180 spawn storm (pij-interesting-capybara, prime pij-broad-rykard) | 23:39 | 02:20 (+1 d) | tmux control mode, one-process Copilot status line, per-site launch counter in ping, /bin/ps + /bin/date; 4 windows; machine 740.6 -> 16.5 new processes/s, Defender 156.6% -> 8.6% |
| 2026-10-29 | Plan 185 daemon CPU and memory (pij-daemon-burn, prime pij-far-jackal) | 08:30 | 17:00 (+1 d, incl. a disk-full stall) | read-only profiles, D1-D6, F1-F6, tap trim + ledgered orphan sweep, private-daemon replay; CPU 62% -> 1.4% of a core, RSS 5.6 GB climbing -> 15 MB flat, spine growth 10/s -> 0 |
| 2026-11-17 | Plan 227 fleet metro map (pij-previous-firefly, prime pij-far-jackal) | 10:01 | 10:35 | `pij fleet-map init\|generate\|open` skill tool, governance/scratch defaults, skill-folder guard, pij-first verdicts, out-of-order track; 10 tests, live pij-fleet smoke |
| 2026-11-19 | Plan 232 proxied tool-list flips (pij-joyous-sailfish, pm-004, prime pij-far-jackal) | 13:27 | 14:40 | toolSearch=false canary (flat 49 tools, one reload residual) plus the fleet's residual flips; sim232.py re-prices egret's windows (achievable +8.5% / -11.2% vs plain, bound +26.3% / +7.3%); per-seat TTL weighed, keep 1h, nothing built; live after-window split by process start; PR #255 |
| 2026-11-20 | F215 + F216 trust prompt at spawn, pij_close by id (pij-compact-bat, pm-001 pij-steady-otter) | 17:05 | 17:21 | trust.rs + 5 live prompt fixtures, fast 409 refusal naming the safe key, `--trust-folder`, `press_key(MenuKey)`; `pij_close` via `POST /v1/close` with typed `kill_pane`, legacy close removed; 8 new Rust + 3 TS tests, all mutation-checked |
| 2026-11-20 | F222 step 1 per-route request counters (pij-compact-bat, pm-001 pij-steady-otter) | 18:35 | 18:35 | `data.requests` in ping: per route template x method x {ok,refused}, `/v1/seats` by query shape; outermost layer, no launch; 3 tests, 3 mutants killed |
