# Plan 187 rulings

| id | topic | ruling |
|---|---|---|
| R1 | Chainglass | Read-only. Never edit, restart or reconfigure it; the fix is in the daemon. |
| R2 | Live daemon | Read-only: daemon.log, read-only sqlite queries and `pij-rs --json ping` only. Never bounce or restart it, and never run a second daemon on the default tmux server or ~/.pij-rs. |
| R3 | Private replay | One seeded store (about 3M rows), reused across runs, with its own port, state dir and private `tmux -L` server. Check `df -h /System/Volumes/Data` before each run and delete run dirs after. |
| R4 | Wire contract | The event frame format, the cursor contract and the CursorBeyondSpine refusal don't change. |
| R5 | Spine | Never delete or rewrite spine rows. |
