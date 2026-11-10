# pij-rival-salmon: F174 (2026-11-10)

**magicWand:** one daemon client. The same "is the daemon there, and is
this failure safe to try again" decision now lives in four places: the Rust
CLI, the `pij` router's health probe, the OMP client and the Copilot client.
Each needed its own copy of the refused-connection check, the schedule and
the list of calls that must answer at once.

**What happened:** a seat's `pij directory` and `pij requests` failed during
a daemon bounce, reported as "about a minute". The machine's own records say
otherwise. The old daemon was still answering at 14:58:19 and gone by
14:58:24.9, and the new one started at 14:58:25.87, so the port was closed
for about a second. The seat's one tool call ran from 14:58:21.9 to
14:58:26.4, and both of its calls failed inside that second. The 2.5-minute
release build ran before the stop, as `just bounce-rs` intends.

**Found on the way:**
- `just bounce-rs` stops the old daemon with SIGTERM, which ends it at once,
  so a request in flight gets a reset rather than a refusal. `pij daemon
  bounce` sends SIGINT and waits for the drain. Nothing proves the drain ends
  while event streams are open, so the recipe is left alone and the reset is
  a named limit.
- The `pij` router probes `/health` before a routed call and turns a refused
  probe into an immediate refusal. So `pij send <seat>` would never have
  reached the Rust client's retry; the probe needed its own wait.
- A blanket retry in an extension client is wrong. Registration and inbox
  loops already back off on their own, and hooks run inside a turn. Only the
  model's tool calls should wait.
