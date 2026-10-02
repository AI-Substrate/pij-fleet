#!/bin/sh
# Claude Stop hook (plan 157): publish the seat idle when Claude ends its turn.
# Managed by pij: the daemon installs it at ~/.pij-rs/claude-stop-pij.sh.
# Stop-hook stdout JSON can block a stop, so this prints nothing, ever, and
# every failure is one hook.log line and exit 0.
set -u
# Nothing on stdout or stderr: the shell's job-kill notices included.
exec >/dev/null 2>/dev/null

[ -n "${TMUX_PANE:-}" ] || exit 0
pij_rs=$(command -v pij-rs 2>/dev/null || :)
[ -z "$pij_rs" ] && [ -x "${HOME:-}/.npm-global/bin/pij-rs" ] && pij_rs=${HOME:-}/.npm-global/bin/pij-rs
[ -n "$pij_rs" ] || exit 0

state_dir=${PIJ_RS_STATE_DIR:-${HOME:-/tmp}/.pij-rs}
log_line() {
    mkdir -p "$state_dir" 2>/dev/null &&
        printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$state_dir/hook.log" 2>/dev/null || :
}

input=""
IFS= read -r input 2>/dev/null || :
# The session is binding evidence, not a requirement: without jq the pane alone
# names the seat.
session=""
command -v jq >/dev/null 2>&1 &&
    { session=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null) || session=""; }

err=$(mktemp "${TMPDIR:-/tmp}/pij-activity.XXXXXX" 2>/dev/null) || exit 0
trap 'rm -f "$err"' EXIT

# Bound the publish: the daemon answers in milliseconds, so a stall means it is
# wedged, and the stop must not wait on it. `timeout` is not on stock macOS.
if [ -n "$session" ]; then
    env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --native-session "$session" --state idle 2>"$err" </dev/null &
else
    env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --state idle 2>"$err" </dev/null &
fi
publish=$!
(sleep "${PIJ_RS_ACTIVITY_TIMEOUT:-3}" && kill "$publish") </dev/null &
watchdog=$!
status=0
wait "$publish" || status=$?
kill "$watchdog" 2>/dev/null || :

[ "$status" -eq 0 ] ||
    log_line "refused:activity pane=$TMUX_PANE state=idle status=$status reason=$(tr '\n' ' ' <"$err")"
exit 0
