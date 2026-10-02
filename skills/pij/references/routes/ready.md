# ready — adopt this seat and wait

> Route module — terminal and inert. Perform only this procedure; never route
> onward or inspect work to anticipate the next instruction.

**Job**: make the current agent reachable, report readiness, and stop. The
prime or operator will push the next turn.

## Procedure

1. **You are an OMP seat: its extension registered you at boot. Reply `Ready.` and
   STOP — run NO pij identity command (no adopt, no inbox register).**
   **You are a Pi seat: its own runtime extension also registered you at boot.
   Reply `Ready.` and STOP with the same no-identity-command rule.**
   Copilot with its native `pij_send` tool is also extension-owned: reply `Ready.`
   and stop without a competing identity/adopt/inbox command.
   Copilot without the native tool but with a pane must report native setup
   unavailable; do not start a competing pull receiver. Without a pane it may
   use the verified external pull path below. Otherwise, detect the delivery owner:
   - With an exact non-empty `$TMUX_PANE`, resolve `<h>` from the current host
     (`claude` or `codex` — NEVER `omp` or `pi`: each runtime's extension
     owns its boot registration; `pij adopt` rejects `--harness pi` by design) and run exactly
     `pij adopt "$TMUX_PANE" --harness <h> ${PIJ_PARENT_ID:+--parent "$PIJ_PARENT_ID"}`.
     Carry the parent whenever the env has one — it is a self-declaration of who
     governs you, validated and persisted, and it is the ONLY moment it can be
     recorded without someone later remembering. Never discover or guess another pane.
   - With no `$TMUX_PANE`, run `pij inbox register --json` from the current
     Claude/Copilot/Codex tool shell. Read the accepted id at the complete v2
     envelope's `data.id`. The bridge checks the established native session
     environment/files; rs observes the matching long-lived harness ancestor
     and stores its `(pid, proc_start)`, never the short-lived CLI's process.
     Missing, conflicting or unverified evidence refuses: do not invent a pane,
     process tuple, seat id or legacy fallback. See § C1 for exact prerequisites.
2. **Verify the write, never the print.** Read `pij whoami --json` and `pij phonehome --json`: the complete v2 data must name this seat and corroborate its binding. A tombstoned identity refuses even if an old pid is live. No `revive --attach` repair is ported; native revive requires an explicit tombstoned seat and relaunches it. After reboot/long gaps, every old pane/pid conclusion is stale.
3. Once verified, reply with exactly:

```text
Ready.
```

4. **Wait through the delivery owner.** In push/extension mode, **STOP.** — ending the turn is the wait;
   never start a competing inbox receiver. In verified external pull
   mode, use `pij inbox --wait` (or `pij inbox --wait 30000 --json` for a finite
   wait). The same native session resolves the registered id without exporting
   `PIJ_SESSION_ID`; claims are acknowledged only after successful output.
   Never poll `pij state`, and never treat paneless pull as native-extension
   push attestation. See [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status)
   for genuinely unported verbs, not a legacy workaround.

## Hard boundary

Do not read plans, briefs, government files, repository docs, git state, task
state, or another route. Do not run a harness boot, inspect peers, spawn, send,
tail, delegate, claim work, or infer what the next task might be. Registration,
the exact readiness reply, and waiting are the entire route.

## Once work arrives

Invariant 12 starts applying the moment you accept a unit of work: `pij report
now "<did>" "<next>"` at its start and again at its finish. Every reply you send
to another agent follows § C10 — Wire discipline (`00-routing.md` § Shared
conventions). A seat adopted
through this route is the likeliest to go stale — it can sit for hours, so its
card is the one most often read while it is the one least often refreshed. Being
watchdog-nudged means it was already stale.
