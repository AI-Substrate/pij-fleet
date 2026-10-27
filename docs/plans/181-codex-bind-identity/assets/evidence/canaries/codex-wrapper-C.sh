#!/bin/sh
# canary C: daemon-hosted TUI, with the seat's own identity set as per-thread shell env overrides
exec /Users/jordanknight/.codex/packages/standalone/releases/0.164.0-aarch64-apple-darwin/bin/codex --remote unix:// \
  -c "shell_environment_policy.set.PIJ_SESSION_ID=\"$PIJ_SESSION_ID\"" \
  -c "shell_environment_policy.set.PIJ_SPAWN_ID=\"$PIJ_SPAWN_ID\"" \
  -c "shell_environment_policy.set.TMUX_PANE=\"$TMUX_PANE\"" "$@"
