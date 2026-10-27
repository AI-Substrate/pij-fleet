# Plan 180 rulings (prime pij-broad-rykard; Vaughan delegated judgement while asleep)

| id | topic | ruling |
|---|---|---|
| R1 | stacking | Stack on #71 (telegram-media, 32cd364); rollback d05776df for every window. (23:41) |
| R2 | status-line cache | No cache: ACCEPTED. A one-refresh bound with the dead-seat mutant beats a TTL. (23:58) |
| R3 | control client session | Accepted with conditions (23:58). Chosen: attach to the first existing session with `-f no-output,ignore-size,read-only`, NOT a dedicated `_pij-ctl` session. Why: nothing in pij reads `session_attached` or client lists (grep of crates, .copilot, .omp, harness, skills), and the global client-attached hook is empty (prime's check, 00:32); ignore-size leaves window sizes alone; a dedicated session would be visible in every session picker. Accepted 00:32. |
| R4 | injection | Every argument single-quoted; an argument containing `'`, `\n` or `\r` forks instead. Unit test plus mutants M5 (quote), M5n (newline) and M5r (carriage return, killed after adding its assertion). |
| R5 | reply correlation | Blocks close only on their own `%begin` T N guard; on a timeout the client is REPLACED (killed), so a late reply can never reach the next caller. Accepted at 00:32 as stronger than per-number discard. |
| R6 | deploys | The prime ran both windows (14:03:27Z, 14:19:21Z) from staged copies at ~/.pij-rs/bin/pij-rs-<sha>, because this seat was mid-turn. Later windows stage the same way. |
| R7 | acceptance | Phase 1 judged alone: pij-daemon exec rate down >=90% (ping counters + spawn-rate.py). The ping counter (C) was not built; residual ps tracked as a follow-up (01:20: "a follow-up task, not tonight"). |
