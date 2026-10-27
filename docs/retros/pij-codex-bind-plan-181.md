# pij-codex-bind: Codex seats bind on spawn and reply as themselves (plan 181)

**magicWand:** a harness-agnostic "who am I, observed" probe that every route
calls, instead of each route trusting the caller's `PIJ_SESSION_ID` and
`TMUX_PANE`. Both values are environment, and environment is inherited by
whatever the harness decides to run commands under. Codex moved tool shells into
a shared, long-lived daemon, and every seat silently became the first one. The
same class of bug is waiting in any harness that grows a server mode: Claude's
agent SDK, Copilot's ACP host. pij's identity should come from what it can
observe (the process holding the conversation) with environment as a hint, for
every harness, not just Codex.

**What shipped:**
- Both defects were reproduced live on main and diagnosed before any fix
  (`docs/plans/181-codex-bind-identity/assets/evidence/diagnosis.md`). The bind
  timeout was not new: pij never had a registration path for a spawned Codex
  seat, and the same `spawn.failed` exists from 2026-09-08. The misattribution
  is a Codex change (0.157+, shared app-server daemon keeps its first launcher's
  environment) meeting pij's trust in environment.
- Spawn runs the Codex thread in-process, suppresses the update menu with
  Codex's own key, and sets identity per thread. A daemon Codex binder binds from
  the thread-writer lock the pane's Codex process holds, verified against tmux
  and the process table, launching nothing on quiet ticks. A Codex caller whose
  thread contradicts its claimed seat is refused by name, never advised to adopt.
  The footer parser rejects the quota warning line.

**difficulties:**
- The machine daemon was rebuilt and bounced by the prime mid-diagnosis
  (07:12:07), between canaries B and C, and one spawn request died with
  "could not reach the daemon". Nothing in the session said a bounce was coming.
  Severity low (the PR touched nothing on this path). `stretch:` a fleet-wide
  "daemon bouncing in N s" FYI before any bounce.
- The rollout file, which an earlier memory said binds a Codex seat, is written
  only at the first turn, so binding from it would deadlock against pre-bind
  delivery holds. The thread-writer lock was found only by listing the live
  TUI's open files. Severity medium: an assumption from memory would have
  shipped a binder that never binds. Encoded in `crates/harnesses/src/codex.rs`
  and its doc comment.
- `unsafe_code = "forbid"` rules out in-process libproc, so on macOS the
  process-file read is one `lsof` launch. R12 was met by bounding when the binder
  looks (only when a Codex thread starts or a pending row appears), not by
  removing the launch. Severity low.
- R12's `spawns` counter and `just spawn-rate` live only on PR #73, so the
  launch baseline ran #73's script from a scratch copy and the "after" reading
  is the prime's to take post-deploy. Severity low.
- One unrelated daemon test
  (`native_transient_session_tombstoned_or_observed_dead_owner_admits_new_address`)
  failed once under full-suite load, then passed 6/6 alone and in the next full
  run. It uses a 4 s request timeout. Severity low; `stretch:` widen or derive it.
- `node_modules/.bin/ddocs` is still absent in a fresh worktree; the global
  `ddocs` was used (fourth plan running).
