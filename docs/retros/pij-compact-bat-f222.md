# pij-compact-bat: F222 daemon CPU (steps 1 and 2)

## What happened

- **Step 1 (#266)** added per-route request counters to `ping`. Round 1 was blocked
  on cost (P266-001): the counters sat behind a mutex and allocated on a route's
  first request. Round 2 built a fixed, immutable table with inline atomics.
- **Step 2** used those counters. Two readings 120 s apart named the callers:
  `GET /v1/seats?scope=local` at 1.37/s, and `POST /v1/whoami` refused at exactly 1.00/s.
- **The cause.** A tmux capture of every pane found 10 Copilot panes reading
  `⛓ unregistered`. All 10 were seats their owners had closed, with their
  processes still running.
  - In 7 of them, the extension host had been respawned since the close. Each
    retried registration every 5 s, reading a 92 KB roster each time.
  - Replaying `chooseRegistration` on the live roster, with each seat's real
    identity, reproduced the retryable error for all 10.
  - A CPU-time slope over 400-call bursts priced each call:

    | Call | Daemon CPU per call |
    |---|---|
    | Roster read | ~64 ms |
    | `whoami`, ok | 0.8 ms |
    | `whoami`, refused | 3 ms |

## Magic wand

A way to see, for every pane, which seat row (if any) owns it and why its status
line says what it says. Today that takes a tmux capture of 164 panes, a sqlite
query and a process tree, joined by hand.

## Difficulties

- D-F222-1: a closed seat whose Copilot keeps running looked like a registration
  bug and cost CPU for good. Fixed in this PR.
- D-F222-2: a seat can't reload itself through a pij control while its control
  context is unarmed. A peer-sent reload is refused the same way.
- The extension test fixtures fed `chooseRegistration` tombstoned rows the
  production roster never contained. Their "re-take a retired same-session id"
  path had never run live. Serving closed rows now would have made it live and
  reopened closed seats, so the extension splits off the closed rows first.
  `registrationRows` does the split.
