# Plan 188 rulings

| id | topic | ruling |
|---|---|---|
| R1 | Chainglass | Read-only. Never edit, restart or call it. |
| R2 | Live daemon | Read-only: daemon.log, read-only sqlite queries, `pij-rs --json ping` and `pij-rs --json state <seat>` for timing. Never bounce or restart it, and never run a second daemon on the default tmux server or ~/.pij-rs. |
| R3 | Private daemon | One seeded store (about 4M rows) reused across runs, with its own port, state dir and private `tmux -L` server. Check `df -h /System/Volumes/Data` before each run; delete run dirs after. |
| R4 | Contract | Existing card fields keep their meaning and content (golden comparison). New fields and the bulk read are additive. |
| R5 | Spine | Never delete or rewrite spine rows. A schema change (for example a new index) needs the prime's approval first, because migrations need a store backup. |
