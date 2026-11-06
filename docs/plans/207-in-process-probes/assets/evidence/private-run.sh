#!/bin/bash
# Plan 207 private-daemon evidence (bp-0005, AC5): temp HOME and state dir, a
# private port, no tmux. Never the machine daemon, never ~/.pij-rs.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints ping readings (pid, spawn counts) at rest, around three FYI reads (each
# reads the UTC offset, #127), and around two paneless registrations over a real
# chain: a `sleep` caller under `node` running a stub at Claude's entry path with
# a --session-id, under this shell (#128, ruling C27).
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-207-XXXXXX)
unset TMUX TMUX_PANE PIJ_SESSION_ID PIJ_PARENT_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID
export HOME="$S/home" PIJ_RS_STATE_DIR="$S/state" PIJ_RS_ADDR="127.0.0.1:$PORT"
mkdir -p "$HOME" "$PIJ_RS_STATE_DIR" "$S/bin"

say() { printf '\n$ %s\n' "$*"; }
reading() { # <label>
  "$BIN" --json ping | python3 -c "
import json, sys
d = json.load(sys.stdin)['data']
print('reading $1: pid=%s total=%s by_op=%s' % (d['pid'], d['spawns']['total'], json.dumps(d['spawns']['by_op'], sort_keys=True)))"
}
post() { # <path> <json>
  curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" -H 'content-type: application/json' \
    -d "$2" "http://$PIJ_RS_ADDR$1"
}

"$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done
echo "private daemon pid=$PID port=$PORT"
sleep 5
reading rest-1; sleep 60; reading rest-2

for i in 1 2 3; do "$BIN" fyi-read --seat pij-nobody --claimed-at 0 >/dev/null 2>&1; done
reading after-3-fyi-reads

# The real chain: this shell -> node running a stub at Claude's entry path (a
# Claude host as the matcher sees it) -> sleep (the caller).
SESSION=00000000-0000-4000-8000-000000000207
ENTRY="$S/node_modules/@anthropic-ai/claude-code/cli.js"
mkdir -p "$(dirname "$ENTRY")"
echo "require('child_process').spawn('/bin/sleep', ['600'], {stdio: 'ignore'}); setInterval(() => {}, 1000);" >"$ENTRY"
node "$ENTRY" --session-id "$SESSION" &
HOST=$!
for _ in $(seq 1 50); do CALLER=$(pgrep -P "$HOST" sleep) && break; sleep 0.1; done
START=$(LC_ALL=C ps -o lstart= -p "$CALLER" | python3 -c "
import sys, time
print(time.strftime('%Y%m%d%H%M%S', time.strptime(sys.stdin.read().strip(), '%a %b %d %H:%M:%S %Y')))")
say "chain (ps -o pid,ppid,ucomm,args)"; LC_ALL=C ps -o pid=,ppid=,ucomm=,args= -p "$CALLER,$HOST"
for n in 1 2; do
  say "paneless register $n (caller pid $CALLER, harness claude, session $SESSION)"
  post /v1/register "{\"id\":\"\",\"harness\":\"claude\",\"folder\":\"$S\",\"pid\":$CALLER,\"proc_start\":$START,\"harness_session\":\"$SESSION\"}" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print('ok' if d.get('ok') else 'refused', (d.get('data') or {}).get('id') or d.get('meta'))"
  reading after-register-$n
done

kill "$CALLER" "$HOST" 2>/dev/null
kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
rm -rf "$S"
