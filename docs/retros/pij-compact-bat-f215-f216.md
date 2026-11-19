# pij-compact-bat — F215 + F216 (trust prompt at spawn, pij_close by id)

**Delivered:**
- F215: the spawn wait recognises Copilot's and Claude's folder-trust prompts (`crates/harnesses/src/trust.rs`, five live fixtures) and refuses at once (HTTP 409), naming the prompt, the folder and the safe key, with the pane left live and the F211 wait open. `pij-rs spawn --trust-folder` answers instead, one key at a time from the highlighted row. `TmuxPort::press_enter` became `press_key(MenuKey)`.
- F216: `pij_close` closes through `POST /v1/close` by seat id under the daemon's guard, and kills the pane the daemon now names typed (`data.kill_pane`). The legacy `PijSession.close` and the two reducers only it used are gone.

**magicWand:** the edit tool should refuse (or at least warn) when a relative path resolves outside the git worktree that the session's recent reads targeted. Seven edits silently landed in the main checkout.

**difficulties:**
- D-F215-1: relative edit paths resolved against the seat's start directory (the main checkout), not the lane's worktree.
- The packet's diagnosis ("cwd outside the project") was not the cause: `pij_close` read the extension's boot-time roster snapshot, which never held a child registered after boot. The daemon roster read is unfiltered.
- A bottom-20-row capture misses both prompts in some panes: Claude draws at the top of a tall pane; Copilot's menu scrolls in a short one. The wait now keeps the whole screen from its one read.
- Claude's prompt defaults to `No, exit`, so a blind "press Enter" answer would close the seat. The answer is read from the highlighted row, never assumed.
