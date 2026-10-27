# Plan 181: canaries torn down, and evidence gathered after the ruling

Follows [diagnosis.md](diagnosis.md), whose canary table said "see below" for
teardown.

## Canaries (R3)

| label | seat | pane | spawn | mode | torn down |
|---|---|---|---|---|---|
| A-untrusted | pij-poised-anteater | %113 | timed out 60 s | in-process | `pij close` + window killed, 07:34 |
| B-daemon | pij-compatible-emu | %114 | timed out 30 s (update menu) | daemon (`--remote unix://`) | same |
| C-daemon-envset | pij-steep-lobster | %116 | timed out 30 s | daemon + per-thread env | same |
| D-daemon-noupdate | pij-applicable-warbler | %121 | timed out 30 s, no update menu | daemon + `check_for_update_on_startup=false` | same |
| E-fixed-argv | pij-continued-marlin | %122 | timed out 30 s (machine daemon has no binder) | the fixed argv: in-process | same |

Five of the six allowed. One further attempt at 07:12:06 never reached the daemon
(bounced mid-request); no seat or pane was created. All five rows read
`tombstone_reason = owner-close` afterwards.

## After the ruling (R8-R12): evidence the fix rests on

- **D:** `check_for_update_on_startup=false` (a top-level field in the Codex
  0.164 binary's config schema) removed the update menu in the mode where B and
  C both showed it.
- **E:** the argv the fixed spawn emits ran in-process (hint `? for shortcuts`, no
  `← for agents`), with no update menu. Its shell carried its own
  `PIJ_SESSION_ID`, `PIJ_SPAWN_ID` and `TMUX_PANE=%122`, and its parent was its
  TUI (`canaries/E-fixed-argv/probe.txt`).
- **The thread is named by a lock the TUI holds from startup.** Before any turn,
  E's TUI (pid 1567, parent 1533 = the `__spawn-child` wrapper) held
  `~/.codex/thread-writer-locks/01a17f40-….lock` and no rollout. The rollout
  appears only at the first turn (Codex writes it lazily), so the binder reads
  the lock, not the rollout; binding from the rollout would wait for a turn that
  pre-bind delivery holds back. After its probe turn E's `CODEX_THREAD_ID` was
  `01a17f40-…`, and A's lock likewise matched A's thread. The shared app-server
  holds the locks of the threads it hosts (4 at 07:27), so a daemon-hosted pane
  yields no thread (`canaries/E-fixed-argv/lsof-thread-locks.txt`).
