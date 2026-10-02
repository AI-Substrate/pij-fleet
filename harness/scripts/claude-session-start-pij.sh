#!/bin/sh
set -u

state_dir=${PIJ_RS_STATE_DIR:-}
if [ -z "$state_dir" ]; then
    [ -n "${HOME:-}" ] || exit 0
    state_dir=$HOME/.pij-rs
fi
mkdir -p "$state_dir" 2>/dev/null || exit 0
log_file=$state_dir/hook.log
log_line() {
    printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$log_file" 2>/dev/null || :
}

if [ -z "${TMUX_PANE:-}" ]; then
    log_line "skip:no-tmux"
    exit 0
fi
# The payload's session_id is only a cross-check: the daemon derives the
# conversation from this Claude process's own record (plan 156 ruling 2).
IFS= read -r input 2>/dev/null || input=""
session=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null) || session=""
adopt() {
    if [ -n "$session" ]; then
        pij-rs adopt "$TMUX_PANE" --harness claude --harness-session "$session" 2>&1
    else
        pij-rs adopt "$TMUX_PANE" --harness claude 2>&1
    fi
}

if pij-rs ping >/dev/null 2>&1; then
    if adopt_output=$(adopt); then
        printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"pij automatically adopted this Claude session."}}'
    else
        log_line "refused:adopt pane=$TMUX_PANE reason=$(printf '%s' "$adopt_output" | tr '\n' ' ')"
    fi
    exit 0
fi

# The daemon is not up yet (a reboot races it). Retry in the background for
# about 30 s so Claude's start is never held; the status line heals after that.
log_line "wait:no-daemon pane=$TMUX_PANE"
(
    tries=${PIJ_RS_HOOK_RETRIES:-30}
    while [ "$tries" -gt 0 ]; do
        sleep 1
        tries=$((tries - 1))
        pij-rs ping >/dev/null 2>&1 || continue
        if adopt_output=$(adopt); then
            log_line "adopted:retry pane=$TMUX_PANE"
        else
            log_line "refused:adopt pane=$TMUX_PANE reason=$(printf '%s' "$adopt_output" | tr '\n' ' ')"
        fi
        exit 0
    done
    log_line "gave-up:no-daemon pane=$TMUX_PANE"
) </dev/null >/dev/null 2>&1 &
exit 0
