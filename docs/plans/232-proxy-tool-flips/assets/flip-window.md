# Proxied tool-list flips, 2026-11-18T05:55:00Z to 2026-11-19T04:28Z (setting 2026-11-19T03:27:46Z)

| regime | sessions | hot requests | same-process tool count changed | same-process tool tokens changed, count same | same-process flip rate | across a restart | of same-process flips, re-wrote >50% | hot no-flip re-wrote >50% | hot write tokens per request | distinct tool counts |
|---|---|---|---|---|---|---|---|---|---|---|
| proxied, before (proxy live, started pre-setting, request pre-setting) | 42 | 2,578 | 67 | 7 | 2.87% | 1 | 64 (86%) | 7 (0.3%) | 7,363 | 49:1236, 50:456, 36:385, 35:243, 48:112, 37:75 |
| proxied, after, process started BEFORE setting (old seats) | 8 | 188 | 2 | 0 | 1.06% | 0 | 2 (100%) | 2 (1.1%) | 4,572 | 48:83, 50:46, 37:31, 49:20, 36:6, 43:2 |
| proxied, after, process started AFTER setting (toolSearch=false) | 17 | 443 | 1 | 4 | 1.13% | 1 | 4 (80%) | 1 (0.2%) | 4,689 | 49:408, 36:35 |
| plain (CAPI), same span | 8 | 609 | 5 | 0 | 0.82% | 0 | 1 (20%) | 1 (0.2%) | 2,595 | 37:320, 50:108, 36:87, 35:82, 52:11, 38:1 |
