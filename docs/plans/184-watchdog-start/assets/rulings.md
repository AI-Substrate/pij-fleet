# Plan 184 rulings

| id | topic | ruling |
|---|---|---|
| R1 | the ask | Vaughan, verbatim via the Tether prime pij-causal-salamander (2026-10-29 17:10 AEDT): "let's add to watchdog a --start 2m --every 20m capability, geta  seat to do it in pij prime". Brief: original-ask.md. |
| R2 | start-phase edge (seat's call, recorded) | The start phase ends at the seat's first `report.now` after arming or the first start-interval nudge, whichever comes first. `started_at` is persisted before the start nudge is sent. The report edge reads `report.now` specifically: `last_event_at` also counts the watchdog's own `watchdog.on`/`watchdog.nudge` events. |
| R3 | --start > --every | Allowed (a long first window, then frequent checks); no new refusal. |
| R4 | re-arm | Identical --start/--every is a no-op that keeps the phase; to restart an identical start window, `off` then `on`. |
