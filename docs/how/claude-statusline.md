# Claude statusline

`pij-rs daemon` installs `harness/scripts/claude-statusline-pij.sh` at `~/.pij-rs/claude-statusline-pij.sh` and points each discovered Claude home's `settings.json#statusLine.command` at it.

The installer is additive: it preserves unrelated settings and hooks, backs up `settings.json` before a change, and copies a resolvable previous statusline script to `<name>.bak-<millis>` before changing the command. Malformed settings are reported and left untouched. Re-running with the managed command is a no-op.

The script resolves `$TMUX_PANE` with `pij-rs whoami --pane` first. An rs hit renders a yellow `rs` tag and never starts the TypeScript CLI. An ordinary rs miss attempts the legacy lookup; if neither lookup resolves the pane and a conversation id is available, the detached heal shows `⛓ registering…`.

An `E-RS-CONVERSATION-LIVE` refusal instead renders a red `⚠ double: <seat> in %N` and suppresses another heal adopt. A coded refusal from the detached heal is remembered for the current conversation; registration or a changed conversation clears that marker. See [Double seat](double-seat.md) for the warning and manual recovery. The other model, context, branch, effort, and rate-limit segments remain unchanged.

Inspect without changing files:

```sh
pij-rs doctor claude-statusline
```
