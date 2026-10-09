# pij-corresponding-lobster — plan 164 (federation, switched on and secured)

## magicWand

A `just dev-daemon <name> <port>` recipe that boots an isolated daemon the way
the live proof needed: its own state dir, HOME, Claude config and private tmux
server, with `TMUX`, `TMUX_PANE` and `PIJ_SESSION_ID` stripped. Two of them
plus `pij-rs peers new-key` would make a two-machine federation proof a
one-liner instead of a hand-built shell harness.

## difficulties

- **severity: medium — a "private" tmux server was not private.** Setting
  `TMUX_TMPDIR` does nothing while `$TMUX` is set: tmux talks to the server
  named in `$TMUX`. The first live-proof run created its session (and adopted
  seats) on the operator's real tmux server. Caught from `list-panes`, killed,
  redone with `env -u TMUX -u TMUX_PANE`. Encode in the dev-daemon recipe above.
- **severity: low — the caller's own `PIJ_SESSION_ID` leaked into the dev
  daemons' CLI calls**, so `pij-rs send` asserted the orchestrating seat's id
  against a store that had never heard of it. Same fix: strip it.
- **severity: low — `arch_drift` fixtures are a frozen `cargo metadata`
  snapshot.** Adding one dependency (`toml` to pij-daemon) fails two fixture
  tests with `StaleRule` until the fixtures gain the same edge. Expected
  (documented in the test), but easy to miss until the workspace run.
