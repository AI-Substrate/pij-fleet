#!/bin/bash
# Plan 195 private-daemon evidence (bp-0007, AC11): temp HOME and
# state dir, a private -L tmux server, a private port, scratch repos. Never the
# machine daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest and around one
# assign (with the window rename), one release, one view and one close of a
# seat that holds a role.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-190-XXXXXX)
SERVER="pij-190-$$"
SEAT=pij-scratch-marten HOLDER=pij-scratch-holder
# This shell's own seat identity must not leak into the private daemon.
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
print('reading $1: pid=%s ddocs=%s total=%s by_op=%s' % (d['pid'], d['ddocs'], d['spawns']['total'], json.dumps(s, sort_keys=True)))"
}

tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
"$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done
echo "private daemon pid=$PID port=$PORT state=$PIJ_RS_STATE_DIR tmux=-L $SERVER"
# The scratch seat binds to the private server's pane (nothing is typed into it).
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" \
  -d "{\"id\":\"$SEAT\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$PANE\"}" -H 'content-type: application/json' \
  "http://$PIJ_RS_ADDR/v1/register" | head -c 160; echo
export TMUX_PANE=$PANE
tmux -L "$SERVER" new-window -d -t bench -n w1 'sleep 100000'
HOLDER_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}')
curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" \
  -d "{\"id\":\"$HOLDER\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$HOLDER_PANE\"}" -H 'content-type: application/json' \
  "http://$PIJ_RS_ADDR/v1/register" | head -c 120; echo

# Scratch repo: main plus a standing prime-governance worktree.
M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo g >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm government

run "$BIN" project create fleet --repo "$M" --prime "$SEAT"
for id in reviewer pm-001; do
  run "$BIN" directory add "$id" --title "$id" --kind general --lifecycle long-lived --holders many \
    --rule "r" --packet "p" --why "setup"
done
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

run "$BIN" directory assign reviewer/114 "$HOLDER" --why "review #114"
reading after-assign
say "tmux window name"; tmux -L "$SERVER" display-message -p -t "$HOLDER_PANE" '#{window_name}'

run "$BIN" directory release reviewer/114 "$HOLDER" --why "merged"
reading after-release

run "$BIN" directory
reading after-view

run "$BIN" directory assign pm-001 "$SEAT" --why "lead"
reading before-close
run "$BIN" close "$SEAT" --reason "done"
reading after-close

say "git -C gov log"; git -C "$G" log --format='%h %s' -n 6
say "pm-001 holders at HEAD"; git -C "$G" show HEAD:.harness/government/community/roles/pm-001/holders.dd.json

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
