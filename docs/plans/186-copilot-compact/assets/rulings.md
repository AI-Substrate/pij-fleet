# Plan 186 rulings

| id | topic | ruling |
|---|---|---|
| R1 | Scope | Copilot gets `compact` only. `new` and `reload` keep refusing. (Jordan 2026-10-30 asked for compact.) |
| R2 | Mechanism | Use the existing pane control path Claude's compact uses: idle, empty composer, typing gate, staged submit, same failure outcomes. No new typing mechanism and no slash text pasted around the gates. This supersedes legacy plan 137 item 8 for compact only. |
| R3 | Messages | Copilot message bodies stay on the native extension; the pointer drain still never claims them. |
| R4 | Canaries | The branch canary runs on an isolated daemon (own port, own state dir, private `tmux -L` server); at most 2 canary Copilot seats you spawn there. The prime repeats it on the live fleet after deploy. Never touch Jordan's Copilot panes or any other prime's seats (for example pij-influential-opossum). Close canaries and kill their windows when done. |
| R5 | PR #91 | PR #91 is reworking staged-submit internals (crates/tmux/src/lib.rs staged submit, crates/core/src/ports.rs). Don't edit them. If #91 merges before you finish, rebase onto main. If you find you need to change staged submit, ask first. |
| R6 | Public repo | Evidence holds pij-generated and canary text only. |
