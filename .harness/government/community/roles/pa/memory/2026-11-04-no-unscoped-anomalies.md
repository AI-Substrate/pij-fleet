# No unscoped `pij anomalies` until #119 is fixed

tags: pa, watchdog, daemon, cost

Hook: each unscoped call makes the daemon read the whole 2.4 GB spine (peak 6.5 GB). Prime's order, 2026-11-04 15:18.

For rounds use instead:
- `pij-rs --json list` → `data.seats[]` (state, pane, folder; contextUsed is empty for Copilot seats, see the footer note).
- `pij-rs --json state <seat>` → `statusPrev`, `statusNext`, `statusAt` (card age), `semanticState` (question/blocked/waiting), `liveness`, `parent`.
- The nudge's own "Needs a look" list already carries quiet, parked and stale-card seats.

The gap: anomaly kinds other than stale cards (they appear only in `pij anomalies`). Told the prime. Lift this rule when #119 closes.
