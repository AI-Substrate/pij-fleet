# Double seat

A **double seat** is one conversation running in two processes: the seat's own
process, and a second copy in a pane that no seat owns. One conversation cannot
be two seats. Peers reach the seat's copy while the human types into the other,
and every pij verb in the second pane fails.

pij always detected this. Plan 174 (pij-fleet#55) makes the detection visible
with one stable code, `E-RS-CONVERSATION-LIVE`, and one notice to the live seat.
The wire contract is in [pij-rs-api.md § Revive](pij-rs-api.md#revive).

## Worked example: 2026-10-15

1. After the reboot, pij revived `pij-different-smelt` in pane `%5`
   (00:57:34Z), resuming its conversation `cc2d2e8f…`.
2. 49 s later, Jordan resumed the same conversation by hand in `%7`
   (`claude --resume prime`). The resume was by name, so the conversation id
   never appeared on the command line.
3. For about an hour, peers messaged `%5` and Jordan talked to `%7`. In `%7`,
   `send`, `spawn` and `bg` failed with "not registered".
4. pij had detected the double the whole time. `%7`'s SessionStart adopt, its
   status-line heal and every prompt hook were refused with "conversation
   cc2d2e8f… is live as seat pij-different-smelt in another process (pane
   %5)". But that text went only to `~/.pij-rs/hook.log`, and `%5` was never
   told.

The same incident now plays out like this:

| Where | What you see |
|---|---|
| `%7` at start | Claude shows a SessionStart warning: `⚠ pij: this pane is a second copy of a live seat. E-RS-CONVERSATION-LIVE conversation … is live as seat pij-different-smelt in pane %5; close one copy, then run pij-rs adopt %7 --harness claude from the copy you keep …`. The model gets the same line as context. |
| `%7` status line | A red `⚠ double: pij-different-smelt in %5` replaces `⛓ registering…`. |
| `%7` every prompt | The UserPromptSubmit hook shows the same warning again, until the double is gone. |
| `%7` `pij send …` | `send: FAILED — E-RS-CONVERSATION-LIVE …` (the live seat, its pane, the fix), not "not registered". |
| `%5` | One message from `pij-bg`: the conversation is also running in pane %7 (pid N), and how to keep %7 instead. |

**The fix:** close one copy. If you keep `%7`, close `%5`, then run
`pij-rs adopt %7 --harness claude` in `%7`. The seat follows its conversation
into `%7`, and the status line returns to `⛓ <seat>`.

## How it is detected

**The witness is Claude's own process record,**
`<claude home>/sessions/<pid>.json`. It holds `sessionId`, `procStart` (UTC)
and `tmux` (`session:@window.%pane`). Claude rewrites it whenever the
conversation changes, including on a named `--resume`. A record counts only
when its pid is alive **and** its observed start matches `procStart`, so a
stale file for a recycled pid proves nothing. Every Claude home is read,
including `~/.claude-alt`.

Three paths use it:

1. **The revive guard** (plan 156) stops pij from making a double. Revive
   refuses with `E-RS-REVIVE-CONVERSATION-LIVE` when the conversation already
   runs elsewhere: in another seat whose process is alive, or in a verified
   Claude record. On 2026-10-15 the guard was correct, because the hand resume
   came second.
2. **The adopt refusal.** Adopting a pane derives its conversation from that
   pane's own record. If a different, live process holds that conversation as a
   seat, adopt refuses with `E-RS-CONVERSATION-LIVE` without changing either
   seat's identity. The first refusal can record and send the notice below.
3. **Stray-pane verbs.** A verb that finds its caller by pane (`whoami --pane`,
   `send`, `bg`, `report`, `spawn` with a role, `activity`, `fyi-claim`) has
   always refused when no live seat owns the pane. Just before that refusal it
   now reads the pane's verified record. If the record's conversation is live as
   another seat, it raises the coded refusal instead. Without such a record,
   the "not registered" text is unchanged, byte for byte. A pane that resolves
   to a seat never reads a record, so the check costs nothing on the healthy
   path. The check fails open: any read or probe error gives the old refusal.

The refusal is HTTP 409. Its `details` hold
`{code, seat, pane, harness_session, pid}` for the live seat. JSON consumers can
branch on `details.code`; the hooks match the same code in CLI text, and the
status line reads JSON details or its saved heal-refusal line.

## How it is shown

- **SessionStart hook** (`claude-session-start-pij.sh`). On a coded adopt
  refusal it prints `{systemMessage, hookSpecificOutput.additionalContext}`
  with the daemon's line, so the human sees it and the model knows it. It still
  writes the `hook.log` line and exits 0. Any other refusal is only logged, as
  before.
- **UserPromptSubmit hook** (`claude-user-prompt-submit-pij.sh`). When
  `fyi-claim` (or `activity`) is refused with the code, it prints the same
  warning on every prompt while the double persists. A registered pane's FYI
  output is unchanged.
- **Status line** (`claude-statusline-pij.sh`). The red segment comes from
  either of two sources:
  - a coded `whoami` refusal, read from `details.seat` and `details.pane`;
  - a coded heal-adopt refusal. The detached heal writes this to
    `~/.pij-rs/heal/pane-<N>.double`: line 1 is the conversation, line 2 is the
    daemon's line.

  While that marker matches the current conversation, the status line skips the
  heal, so a known double never fires a second adopt. The marker is removed when
  `whoami` resolves the pane to a seat (for example after the manual adopt above)
  or when the conversation changes.
- **The notice to the live seat.** On the first coded refusal for a pair (live
  seat, stray pid + start), the daemon does two things:
  - It writes `seat.double-detected` on the live seat's spine, with payload
    `{msg_id, seat, stray_pane, stray_pid, proc_start, harness_session, body}`.
  - It delivers one message from `pij-bg` through the ordinary path. Cold-wake
    rules apply, so a cold seat gets it as an FYI for its next turn.

  The event is the dedupe record, keyed by the stable id
  `double-seat-<seat>-<pid>-<start>`, so a restarted daemon does not resend.

## The dedupe is a brake, not a policy

Ask what removing it does. Without the dedupe lookup the daemon sends the
**same notice more often**, never a different one. So it can only make sending
more conservative, and it fails safe: a dedupe read that errors counts as
already sent, and a failed event write sends nothing, because sending without
a record would repeat on every verb. Two simultaneous first refusals can both
pass the lookup. They then share one msg_id, and delivery admits a msg_id once,
so the live seat still gets one message; only the audit event may be duplicated.

## Limits

- The record witness exists only for **Claude**. For other harnesses, only the
  adopt check raises the code, and only when it can derive the conversation
  (`--harness-session` from inside the pane's process tree).
- Seats on another machine are never treated as the live copy. A local pid says
  nothing about a remote process.
- A paneless stray (Claude outside tmux) cannot call pane-resolved verbs, so it
  is seen only by revive.

## Proof

- Daemon: `crates/daemon/src/http/double_seat_tests.rs` (fixture Claude homes,
  fake tmux and liveness, private ports). Includes the incident end to end.
- Scripts: `crates/harnesses/tests/claude_heal_scripts.rs` (a fake `pij-rs` on
  `PATH`, private `HOME`).
- Plan, receipts and mutations: `docs/plans/174-double-seat-warning/`.
