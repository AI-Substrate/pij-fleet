#!/bin/sh
# Claude UserPromptSubmit hook (plans 157, 158): publish the seat working, then
# claim its held FYIs and hand the daemon's block to Claude verbatim as
# additional prompt context.
# Managed by pij: the daemon installs it at ~/.pij-rs/claude-user-prompt-submit-pij.sh.
# It must never block the human's prompt: every failure prints nothing, exits 0.
set -u
# Nothing on stderr either: the shell's job-kill notices included.
exec 2>/dev/null

[ -n "${TMUX_PANE:-}" ] || exit 0
# The claim is destructive, so both tools must be present before it runs.
command -v jq >/dev/null 2>&1 || exit 0
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
session=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null) || session=""

out=$(mktemp "${TMPDIR:-/tmp}/pij-fyi-claim.XXXXXX" 2>/dev/null) || exit 0
err=$(mktemp "${TMPDIR:-/tmp}/pij-fyi-claim.XXXXXX" 2>/dev/null) || { rm -f "$out"; exit 0; }
trap 'rm -f "$out" "$err"' EXIT

# Run "$@" with stdout to $out and stderr to $err, bounded by $1 seconds: the
# daemon answers in milliseconds, so a stall means it is wedged, and the prompt
# must not wait on it. `timeout` is not on stock macOS. Sets $status.
bounded() {
    limit=$1
    shift
    "$@" >"$out" 2>"$err" </dev/null &
    job=$!
    (sleep "$limit" && kill "$job") >/dev/null 2>&1 </dev/null &
    watchdog=$!
    status=0
    wait "$job" || status=$?
    kill "$watchdog" 2>/dev/null || :
}

# Plan 157: the prompt means Claude is working. Publishing is best effort; the
# claim runs whatever it says.
if [ -n "$session" ]; then
    bounded "${PIJ_RS_ACTIVITY_TIMEOUT:-3}" env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --native-session "$session" --state working
else
    bounded "${PIJ_RS_ACTIVITY_TIMEOUT:-3}" env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --state working
fi
[ "$status" -eq 0 ] ||
    log_line "refused:activity pane=$TMUX_PANE state=working status=$status reason=$(tr '\n' ' ' <"$err")"

if [ -n "$session" ]; then
    bounded "${PIJ_RS_FYI_CLAIM_TIMEOUT:-3}" env -u PIJ_SESSION_ID "$pij_rs" fyi-claim --pane "$TMUX_PANE" --native-session "$session" --via hook:claude
else
    bounded "${PIJ_RS_FYI_CLAIM_TIMEOUT:-3}" env -u PIJ_SESSION_ID "$pij_rs" fyi-claim --pane "$TMUX_PANE" --via hook:claude
fi

if [ "$status" -ne 0 ]; then
    log_line "refused:fyi-claim pane=$TMUX_PANE status=$status reason=$(tr '\n' ' ' <"$err")"
    exit 0
fi
[ -s "$out" ] || exit 0
# Command substitution drops only the CLI's trailing newline; the block itself
# passes through byte for byte.
block=$(cat "$out")
[ -n "$block" ] || exit 0
printf '%s' "$block" | jq -cRs '{hookSpecificOutput:{hookEventName:"UserPromptSubmit",additionalContext:.}}' 2>/dev/null || :
exit 0
