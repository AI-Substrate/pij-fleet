# Claude statusline

`pij-rs daemon` installs `harness/scripts/claude-statusline-pij.sh` at `~/.pij-rs/claude-statusline-pij.sh` and points each discovered Claude home's `settings.json#statusLine.command` at it.

The installer is additive: it preserves unrelated settings and hooks, backs up `settings.json` before a change, and copies a resolvable previous statusline script to `<name>.bak-<millis>` before changing the command. Malformed settings are reported and left untouched. Re-running with the managed command is a no-op.

The script resolves `$TMUX_PANE` with `pij-rs whoami --pane` first. An rs hit renders a yellow `rs` tag and never starts the TypeScript CLI. An rs miss runs `PIJ_DAEMON_GENERATION=legacy pij whoami` and renders a dim `legacy` tag. An unregistered pane has no pij segment; the other model, context, branch, effort, and rate-limit segments remain.

Inspect without changing files:

```sh
pij-rs doctor claude-statusline
```
