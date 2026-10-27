#!/bin/sh
# canary E: the argv plan 181's spawn emits (the daemon's plan supplies the rest)
exec /Users/jordanknight/.codex/packages/standalone/releases/0.164.0-aarch64-apple-darwin/bin/codex \
  -c features.daemon_auto_start=false -c check_for_update_on_startup=false \
  -c "shell_environment_policy.set.PIJ_SESSION_ID=\"$PIJ_SESSION_ID\"" \
  -c "shell_environment_policy.set.PIJ_SPAWN_ID=\"$PIJ_SPAWN_ID\"" \
  -c "shell_environment_policy.set.TMUX_PANE=\"$TMUX_PANE\"" "$@"
