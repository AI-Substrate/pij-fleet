#!/bin/sh
# Claude Stop hook (plan 157): publish the seat idle when Claude ends its turn.
# Plan 226: when the reply holds a pij fence, the same single pij-rs launch is
# `hook claude-stop` instead. It runs a closing pij block, publishes idle unless
# the seat gets a follow-up turn, and answers the hook: one line for the human,
# or a block that gives the seat one more turn.
# Managed by pij: the daemon installs it at ~/.pij-rs/claude-stop-pij.sh.
# Stop-hook stdout JSON can block a stop, so this prints only that answer, and
# every failure is one hook.log line and exit 0.
set -u
# Every file this writes is the user's alone (S213-001): the reply text and
# the daemon's answer go in a private directory, and nothing is group- or
# world-readable even where TMPDIR is a shared /tmp.
umask 077
# Nothing on stdout or stderr but the answer (fd 3): the shell's job-kill
# notices included. No background child gets fd 3 (`3>&-`): Claude reads the
# hook's stdout to its end, and from 2.1.322 it refuses a block answer when a
# leftover child (the watchdog's sleep) still holds it open.
exec 3>&1 >/dev/null 2>/dev/null

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

work=$(mktemp -d "${TMPDIR:-/tmp}/pij-stop.XXXXXX" 2>/dev/null) || exit 0
trap 'rm -rf "$work"' EXIT
err=$work/err

# Bound the call: the daemon answers in milliseconds, so a stall means it is
# wedged, and the stop must not wait on it. `timeout` is not on stock macOS.
case $input in
*'```pij'*)
    # The daemon parses for real; a fence mid-reply only publishes idle there.
    printf '%s\n' "$input" >"$work/in" || exit 0
    env -u PIJ_SESSION_ID "$pij_rs" hook claude-stop --pane "$TMUX_PANE" <"$work/in" >"$work/out" 2>"$err" 3>&- &
    publish=$!
    bound=${PIJ_RS_TURN_END_TIMEOUT:-35}
    ;;
*)
    # The session is binding evidence, not a requirement: without jq the pane
    # alone names the seat.
    session=""
    command -v jq >/dev/null 2>&1 &&
        { session=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null) || session=""; }
    if [ -n "$session" ]; then
        env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --native-session "$session" --state idle 2>"$err" </dev/null 3>&- &
    else
        env -u PIJ_SESSION_ID "$pij_rs" activity --pane "$TMUX_PANE" --state idle 2>"$err" </dev/null 3>&- &
    fi
    publish=$!
    bound=${PIJ_RS_ACTIVITY_TIMEOUT:-3}
    ;;
esac
(sleep "$bound" && kill "$publish") </dev/null 3>&- &
watchdog=$!
status=0
wait "$publish" || status=$?
kill "$watchdog" 2>/dev/null || :

if [ "$status" -ne 0 ]; then
    if [ -e "$work/in" ]; then
        log_line "refused:turn-end pane=$TMUX_PANE status=$status reason=$(tr '\n' ' ' <"$err")"
    else
        log_line "refused:activity pane=$TMUX_PANE state=idle status=$status reason=$(tr '\n' ' ' <"$err")"
    fi
fi
# The answer is one JSON line, or nothing.
answer=""
[ -s "$work/out" ] && IFS= read -r answer <"$work/out"
[ -n "$answer" ] && printf '%s\n' "$answer" >&3
exit 0
