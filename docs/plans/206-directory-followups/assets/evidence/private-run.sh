#!/bin/bash
# Plan 206 private-daemon evidence (tk-0007, ac-0006): temp HOME and state dir,
# a private -L tmux server, a private port, a scratch main checkout with its
# own prime-governance worktree. Never the machine daemon or ~/.pij-rs.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints ping readings (pid, spawn counts) at rest and around one add, edit,
# retire, merge, a send to the retired role, and a retire refused for a queued
# request then landed with --drop-queued, so each verb's launches can be
# compared with add's.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-206-XXXXXX)
SERVER="pij-206-$$"
SEAT=pij-scratch-wildcat
unset TMUX TMUX_PANE PIJ_SESSION_ID PIJ_PARENT_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID
export HOME="$S/home" PIJ_RS_STATE_DIR="$S/state" PIJ_TMUX_SERVER="$SERVER" PIJ_RS_ADDR="127.0.0.1:$PORT"
mkdir -p "$HOME" "$PIJ_RS_STATE_DIR"
git config --global user.name scratch
git config --global user.email scratch@example.invalid
git config --global commit.gpgsign false
git config --global init.defaultBranch main

say() { printf '\n$ %s\n' "$*"; }
run() { say "$*"; "$@"; echo "exit=$?"; }
reading() { # <label>
  "$BIN" --json ping | python3 -c "
import json, sys
d = json.load(sys.stdin)['data']
s = d['spawns']['by_op']
print('reading $1: pid=%s total=%s by_op=%s' % (d['pid'], d['spawns']['total'], json.dumps(s, sort_keys=True)))"
}

tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
"$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done
echo "private daemon pid=$PID port=$PORT tmux=-L $SERVER"
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" \
  -d "{\"id\":\"$SEAT\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$PANE\"}" -H 'content-type: application/json' \
  "http://$PIJ_RS_ADDR/v1/register" | head -c 100; echo
export TMUX_PANE=$PANE

# Scratch main checkout with a prime-governance worktree, as the tests build it.
M="$S/main" G="$S/gov"
mkdir -p "$M" && git -C "$M" init -q && echo main >"$M/README.md" && git -C "$M" add README.md && git -C "$M" commit -qm main
git -C "$M" update-ref refs/heads/prime-governance "$(git -C "$M" commit-tree "$(git -C "$M" mktree </dev/null)" -m governance)"
git -C "$M" worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo government >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm government
cd "$M" || exit 1

run "$BIN" project create demo --repo "$M" --prime "$SEAT"
sleep 10  # let the pane observer finish attaching to the seat's pane
reading rest-1; sleep 60; reading rest-2

for id in qa review sec ops; do
  run "$BIN" directory add "$id" --title "$id" --kind general --lifecycle long-lived --holders one --rule r --packet p --why start
  reading "after-add-$id"
done
run "$BIN" directory edit qa --set summary=edited --why "cost run"
reading after-edit
run "$BIN" directory retire qa --why "cost run"
reading after-retire
run "$BIN" directory merge review --into sec --why "cost run"
reading after-merge
run "$BIN" send --to role:qa --body "to a retired role"
reading after-send-retired
run "$BIN" send --to role:ops --body "queued: ops has no holder"
reading after-send-queued
run "$BIN" directory retire ops --why "cost run"
reading after-retire-refused
run "$BIN" directory retire ops --why "cost run" --drop-queued
reading after-retire-drop-queued
run "$BIN" directory
reading after-view
git -C "$G" log --oneline | head -8

kill "$PID"; wait "$PID" 2>/dev/null
tmux -L "$SERVER" kill-server
