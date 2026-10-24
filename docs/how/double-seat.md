# Double seats: one conversation, two processes

A **double seat** is one harness conversation running in two processes while
pij knows it as one seat. Peers reach one copy; the human types into the other;
neither side can tell. pij detects it, refuses to make the second copy a seat,
and says so everywhere a human or an agent will look. It never closes either
copy: the human decides which one stays. Plan 174, pij-fleet#55.

## Worked example: 2026-10-15

After a Mac reboot, `pij revive` relaunched `pij-different-smelt` in pane %5 at
00:57:34Z, resuming its Claude conversation `cc2d2e8f…`. Forty-nine seconds
later Jordan resumed the same conversation by hand in pane %7 with
`claude --resume prime`. A named resume puts no id on the command line, so
nothing in tmux showed the two were one conversation.

For about an hour peers delivered to %5 while Jordan talked to %7, and every
`send`, `spawn` and `bg` from %7 failed with "not registered". The daemon had
known from the first second. %7's SessionStart adopt, its status-line heal and
every prompt hook were refused with "conversation cc2d2e8f… is live as seat
pij-different-smelt in another process (pane %5)". But that text reached only
`~/.pij-rs/hook.log`, and %5 was never told. Revive's own guard was correct: it
ran 49 s before the hand resume, when there was no second copy yet.

With plan 174, the same hour looks like this:

1. %7's SessionStart hook shows the human `⚠ pij double seat: E-RS-CONVERSATION-LIVE
   conversation … is live as seat pij-different-smelt in another process (pane %5);
   close one copy, then run pij-rs adopt %7 --harness claude from the copy you keep`.
   The model gets the same text as context.
2. %7's status line shows a red `⚠ double: pij-different-smelt in %5`, not
   `⛓ registering…`, and stops firing heal adopts that can only be refused.
3. Every prompt in %7 repeats the warning until the double clears.
4. Any pij verb from %7 (`send`, `spawn`, `bg`, `whoami`, …) refuses with the
   same code and text, not "not registered".
5. %5 receives one message: its conversation is also running in %7, with the
   pid and the fix.

## The witness

Claude writes `<claude home>/sessions/<pid>.json` for every interactive process
(`pid`, `sessionId`, `procStart` in UTC, and `tmux` as `session:@window.%pane`),
and rewrites `sessionId` on a named `--resume`. It is the only record that names
the conversation a process is running, which is why detection covers Claude
only. Other harnesses are caught at adopt when they pass a verified
`--harness-session`.

A record counts only when its pid's **observed** start (the liveness port)
matches the start it recorded. Pids recycle at boot, so a stale file for a
reused pid derives nothing.

## One helper, two callers

`crates/daemon/src/http/double_seat.rs` holds the detection once:

- `conversation_live_as_seat(state, harness, session, skip)`: the live seat
  (not tombstoned, recording a process that is not observed gone) holding the
  conversation, other than the ones `skip` names. An unknown liveness answer
  counts as running, because every caller refuses on a hit.
- `live_claude_records(state, keep)`: the record witness above.

Its callers:

| caller | asks | on a hit |
| --- | --- | --- |
| `adopt` | is the derived conversation live as a seat in a process other than this pane's? | `E-RS-CONVERSATION-LIVE` |
| pane resolution (`resolve_seat`, the hooks' `bound_seat`) | does this pane, which no running seat owns, have its own record naming a conversation live as a seat in another pane? | `E-RS-CONVERSATION-LIVE` |

"No running seat owns the pane" covers three shapes, all seven verbs alike:
no live row names the pane; only retired (tombstoned) rows do, and the double
outranks the tombstone refusal; or the store-live row that names it records a
process that is gone. Pane resolution reads the records only then, so a verb
from a pane whose seat is running never pays for the scan. A pane with no record, a
mismatched start, or a conversation no other seat holds keeps the plain
"not registered" text.

The code string is defined once, `pij_core::double_seat::CONVERSATION_LIVE_CODE`.
The shell scripts match it as a literal, and
`scripts_spell_the_conversation_live_code_as_rust_defines_it` pins each script's
literal to the constant.

**Revive is not a caller.** Its pre-launch guard, `E-RS-REVIVE-CONVERSATION-LIVE`,
keeps its own original rule (a plan 174 non-goal): any other row holding the
conversation, tombstoned included, whose process is observed `Active`, then the
same Claude record check. A failed probe does not refuse there.
`revive_conversation_guard_keeps_its_original_rule` pins it.

## The refusal

```text
E-RS-CONVERSATION-LIVE conversation `<id>` is live as seat `<seat>` in another process (pane %5); close one copy, then run `pij-rs adopt %7 --harness claude` from the copy you keep — one conversation cannot be two seats
```

HTTP 409; `details` is `{code, seat, pane, harness_session, pid, stray_pane,
stray_pid}`. The full contract is in
[pij-rs-api.md](pij-rs-api.md#double-seat-e-rs-conversation-live-plan-174).

## The human-facing surfaces

| surface | script | on the coded refusal |
| --- | --- | --- |
| SessionStart | `claude-session-start-pij.sh` | hook JSON with `systemMessage` (human) and `additionalContext` (model); still logs to `hook.log` |
| UserPromptSubmit | `claude-user-prompt-submit-pij.sh` | the same JSON on every prompt, from the refusal its `activity` or `fyi-claim` call already received; nothing once the double clears |
| status line | `claude-statusline-pij.sh` | red `⚠ double: <seat> in %N` in place of the seat tag or `⛓ registering…`; no heal adopt. Two sources: a coded `whoami` refusal, or a heal adopt refused with the code (the pane is another seat's, recording a different conversation). The refused heal leaves `heal/pane-<N>.double` naming this conversation; while the payload names it, the line stays red and no further adopt fires. It clears once `whoami` answers with this conversation, or the conversation changes |

Any other refusal keeps its old behaviour (log only), every script exits 0, and
every daemon call keeps its existing bound. A registered pane's FYI block is
passed through byte for byte as before.

## The notice to the live seat

The first coded refusal for a pair (live seat, stray pid plus its start) writes a
spine `seat.double-detected` event, then sends the live seat one `pij-bg` message
through the same send-or-hold path as the park notice: delivered where `pij send`
would be, held as an FYI where the cold-wake guard would refuse a send.

The spine event is the dedupe record, so repeats from any verb, and daemon
restarts, send nothing more. A new stray process (Claude restarted in the stray
pane) is a new pair and gets one new notice. The dedupe is a **brake**, not a
policy: removing it only sends more notices, never different ones. So an
unreadable spine counts as already sent, a failed audit write sends nothing, and
one lock keeps two simultaneous refusals (a SessionStart adopt and a status-line
render) from both sending.

## The fix

Close one copy, then run `pij-rs adopt "$TMUX_PANE" --harness claude` from the
copy you keep. If you keep the seat's original pane, nothing else is needed: it
is still the seat. If you keep the other one, adopt moves the seat to it once
the first process is gone.

## Proof

`crates/daemon/src/http/double_seat_tests.rs` (adopt refusal, every stray-pane
verb, all seven verbs for a dead store-live owner and for a pane with retired
history, unchanged text for unproven strays, no scan on a resolved pane, a
tombstoned or dead holder is not a live copy, one notice per pair across a
restart, one detection for two simultaneous first refusals, fail-safe dedupe
read), `revive_conversation_guard_keeps_its_original_rule` in
`crates/daemon/tests/revive_http.rs`,
`crates/daemon/tests/double_seat_http.rs` (the incident end to end on the real
SQLite store, with a real restart) and `crates/harnesses/tests/claude_heal_scripts.rs`
(the three scripts). Run them with:

```sh
cargo test -p pij-daemon double_seat
cargo test -p pij-harnesses --test claude_heal_scripts
```
