# Plan 186 canary: Copilot compact on an isolated branch daemon

Run 2026-10-30 05:08–05:18 UTC by pij-copilot-compact. Branch daemon built from
this worktree (`target/debug/pij-rs`, production code as committed).

## Isolation (R4)

| what | value |
|---|---|
| daemon | `pij-rs daemon --bind 127.0.0.1:17186 --state-dir $SCRATCH/canary/state` |
| daemon env | `HOME=$SCRATCH/canary/home` (boot writes Claude hook settings into every Claude home under `HOME`; the daemon log shows only `$SCRATCH/canary/home/.claude` touched), `PIJ_TMUX_SERVER=c186`, `PIJ_TELEGRAM_ENV` unset, `PIJ_SESSION_ID` unset |
| tmux | private server `tmux -L c186`, started with `PIJ_RS_ADDR`/`PIJ_RS_STATE_DIR` pointing at the canary daemon so the Copilot extension registered there, not on 7461 |
| machine daemon | read-only `pij-rs --json ping` only |
| seats | `pij-c186-a`: Copilot, stopped at Copilot's folder-trust dialog, window killed, never registered. `pij-c186-b`: Copilot, registered with `native_extension_delivery: true`, extension build `d459ddafae` (main; the extension is unchanged by this plan) |

Two manual steps came from the prime, not from me: answering B's folder-trust
dialog ("Yes", this session only), and pasting B's first prompt. The isolated
daemon accepts a send only from a seat registered on it, and B was the only one.
Every compact was sent by B itself through the branch CLI and typed into B's pane
by the branch daemon's drain.

The prompt asked B to explain TCP congestion control (context), then to send
itself `--command compact` mid-turn (`c186-busy`), and to schedule a second
compact 150 s later from a detached shell (`c186-idle`).

## Delivery events (canary daemon `pij-rs tail --json`)

```
1793337237427 seat.activity {"state":"working"}
1793337244749 seat.activity {"state":"idle"}
1793337244752 seat.activity {"state":"working"}
1793337281626 seat.activity {"state":"idle"}
1793337281630 seat.activity {"state":"working"}
1793337291489 seat.activity {"state":"idle"}
1793337291491 seat.activity {"state":"working"}
1793337298016 message.pushed {"msg_id":"c186-busy","from":"pij-c186-b","body":"","command":"compact"}
1793337298016 delivery.outcome {"msg_id":"c186-busy","outcome":{"outcome":"queued","reason":"pty","next_retry_at":0},"transport":"pty"}
1793337298609 seat.activity {"state":"idle"}
1793337298611 seat.activity {"state":"working"}
1793337299066 delivery.held {"deferral_count":1,"draft_sha":null,"job_id":1,"last_edit_at":null,"msg_id":"c186-busy","reason":"recipient-busy","reason_changes":0,"remaining_ms":null,"seat":"pij-c186-b","since_ms":1793337299066}
1793337322585 seat.activity {"state":"idle"}
1793337322587 seat.activity {"state":"working"}
1793337325478 seat.activity {"state":"idle"}
1793337344140 delivery.released {"at_ms":1793337344140,"msg_id":"c186-busy","seat":"pij-c186-b"}
1793337344140 delivery.outcome {"msg_id":"c186-busy","outcome":{"outcome":"delivered","origin":"injected-to-transport"},"transport":"pty"}
1793337440778 message.pushed {"msg_id":"c186-idle","from":"pij-c186-b","body":"","command":"compact"}
1793337440778 delivery.outcome {"msg_id":"c186-idle","outcome":{"outcome":"queued","reason":"pty","next_retry_at":0},"transport":"pty"}
1793337443117 seat.activity {"state":"working"}
1793337444078 delivery.held {"deferral_count":1,"draft_sha":null,"job_id":2,"last_edit_at":null,"msg_id":"c186-idle","reason":"recipient-busy","reason_changes":0,"remaining_ms":null,"seat":"pij-c186-b","since_ms":1793337444078}
1793337449890 seat.activity {"state":"idle"}
1793337454146 delivery.released {"at_ms":1793337454146,"msg_id":"c186-idle","seat":"pij-c186-b"}
1793337454146 delivery.outcome {"msg_id":"c186-idle","outcome":{"outcome":"delivered","origin":"injected-to-transport"},"transport":"pty"}
```

Queue rows at the end (`sqlite3 -readonly .../pij.sqlite`):

```
id|kind|serial_key|state|attempt|deferral_count|deferral_reason|dedupe_key
1|delivery-control:pij-c186-b|pij-c186-b|done|5|5|recipient-busy|c186-busy
2|delivery-control:pij-c186-b|pij-c186-b|done|2|2|recipient-busy|c186-idle
```

- **Busy case (`c186-busy`).** Admitted at ...298016 while B was mid-turn:
  `queued`, reason `pty`, on `delivery-control:pij-c186-b`. Held `recipient-busy`
  for five drain passes, none of which touched tmux. B went idle at ...325478; the
  next due retry typed `/compact` at ...344140 (18.7 s later; the busy backoff had
  grown to 16 s). Copilot then compacted.
- **Second case (`c186-idle`).** B was idle when it was admitted (...440778), but
  the detached shell that sent it finishing woke B into a new turn 2.3 s later
  (...443117), before the next drain pass. So it was held `recipient-busy` once and
  typed 4.3 s after B went idle again. **A compact admitted to an already-idle
  seat and typed on the very next pass was not reproduced live**; it is covered by
  `copilot_compact_is_typed_through_the_pane_gate_and_bodies_stay_native`.
- No message body was ever claimed by the drain: the only rows are the two
  controls, both on the control kind.

## Copilot's own compaction events (B's `events.jsonl`)

```
2026-10-30T05:15:44.148Z session.compaction_start {"trigger": "manual", "currentTokens": 37558}
2026-10-30T05:16:15.895Z session.compaction_complete {"trigger": "manual", "success": true, "outcome": "applied", "preCompactionTokens": 37558, "postCompactionTokens": 30530, "tokensRemoved": 7028, "messagesRemoved": 15}
2026-10-30T05:17:34.151Z session.compaction_start {"trigger": "manual", "currentTokens": 31166}
2026-10-30T05:17:56.120Z session.compaction_complete {"trigger": "manual", "success": true, "outcome": "applied", "preCompactionTokens": 31166, "postCompactionTokens": 30795, "tokensRemoved": 371, "messagesRemoved": 5}
```

## Pane, before and after (frames every ~5 s from `tmux -L c186 capture-pane`)

Before the first compact: idle, composer empty, 38k.

```
--- frame t095
 ● DONE                                                                        ┃
                                                                               ┃
 $SCRATCH/canary/work                       Session: 13.41 AIC used
────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
 1 background /tasks · Interactive · Allow All · / c GPT-6.1 Sol · 922K context
4% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 38k/922k •
--- frame t115
                                                                               ┃
◉ Compacting conversation history (4s)
 $SCRATCH/canary/work                       Session: 13.41 AIC used
────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
 1 background /tasks · Interactive · Allow All · / c GPT-6.1 Sol · 922K context
4% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 38k/922k •
--- frame t145
   └ Use /session checkpoints 1 to view the compaction summary.                ┃
                                                                               ┃
 $SCRATCH/canary/work                       Session: 13.41 AIC used
────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
 1 background /tasks · Interactive · Allow All · / c GPT-6.1 Sol · 922K context
3% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 30k/922k •
--- frame t220
                                                                               ┃
◉ Compacting conversation history (5s)
 $SCRATCH/canary/work                       Session: 20.83 AIC used
────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
 Interactive · Allow All · / commands · tab next tab GPT-6.1 Sol · 922K context
3% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 31k/922k •
--- frame t240
   └ Use /session checkpoints 2 to view the compaction summary.                ┃
                                                                               ┃
 $SCRATCH/canary/work                       Session: 20.83 AIC used
────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────
 Interactive · Allow All · / commands · tab next tab GPT-6.1 Sol · 922K context
3% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 30k/922k •
```

## Cost (ac-0004)

`pij-rs --json ping | jq .data.spawns` on the canary daemon, before B's first turn
and after the second compaction:

```
before: {"total":13,"by_op":{"date.utc-offset":1,"ps.registration-native-process":3,"ps.sidecar-process-group":1,"tmux.control-client":1,"tmux.new-window":2,"tmux.pipe-pane attach":2,"tmux.pipe-pane detach":1,"tmux.set-options tap owner":2}}
after:  {"total":39,"by_op":{"date.utc-offset":3,"ps.registration-native-process":3,"ps.sidecar-process-group":1,"ps.tmux-harness-process-tree":4,"tmux.atomically reserve staged submit":2,"tmux.authorize staged submit commit":2,"tmux.control-client":1,"tmux.load submit buffer":6,"tmux.new-window":2,"tmux.paste owned submit buffer":6,"tmux.pipe-pane attach":2,"tmux.pipe-pane detach":1,"tmux.restore pane input and release ownership":2,"tmux.set-options tap owner":2,"tmux.submit authorized staged body":2}}
```

Delta for two admitted and delivered compacts, per op:

| op | delta | per compact | where |
|---|---|---|---|
| `tmux.atomically reserve staged submit` | 2 | 1 | staged submit (shared with Claude's compact) |
| `tmux.load submit buffer` | 6 | 3 | staged submit |
| `tmux.paste owned submit buffer` | 6 | 3 | staged submit |
| `tmux.authorize staged submit commit` | 2 | 1 | staged submit |
| `tmux.submit authorized staged body` | 2 | 1 | staged submit |
| `tmux.restore pane input and release ownership` | 2 | 1 | staged submit |
| `ps.tmux-harness-process-tree` | 4 | 2 | `/v1/send` control caller identity |
| `date.utc-offset` | 2 | 1 | admission |

13 launches per delivered compact, all from existing call sites that Claude's
compact already uses (`submit_command_claim` → `InteractionGate::submit_with_owned_input`,
and `send_control`). **The seven `recipient-busy` holds launched nothing**: the
brake reads the registry row the drain already loaded, before the gate. No new
loop, poll, timer or launch site; the drain cadence is unchanged, and its
per-tick cost for a Copilot seat with nothing queued is one `peek` that now
includes `delivery-control:<seat>` (an indexed SQL read, no launch).

**After review (S1, S3), still no launches.** The drain's `peek` now also lists
paneless Copilot seats' control kind: the same single indexed read, one more
kind. A Copilot control that passes the first idle read now waits 250 ms and
re-reads its registry row before typing. That is one SQL read, and it adds
250 ms to that drain pass only when a Copilot control is about to be typed. A
`working` seat still holds at once, with no wait.

Machine daemon, read-only, before and after (moved by the live fleet, not by
this canary, which never addressed it):

```
{"total":61606}
{"total":61790}
```

## Residual risk seen live

B publishes `idle` then `working` 2–4 ms apart at every step inside a turn (for
example ...244749 idle, ...244752 working). A drain pass that read the registry
inside that window would see `idle` and type `/compact` mid-turn. Over this run
none did: every pass during a turn held. **Closed after review (S3):** the brake
now needs `idle` on two registry reads 250 ms apart, so a blip of a few
milliseconds holds the compact (`copilot_compact_waits_out_an_idle_blip_inside_a_turn`,
mutant M7). The canary above ran before that change.

## Teardown

`kill-window` on B, `tmux -L c186 kill-server` (`no server running on
/private/tmp/tmux-501/c186`), daemon stopped (exit 143), `lsof -iTCP:17186` empty,
B's Copilot process gone.
