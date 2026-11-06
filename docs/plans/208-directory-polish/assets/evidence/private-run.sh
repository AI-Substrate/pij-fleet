#!/bin/bash
# Plan 208 private-daemon evidence (bp-0008, AC8): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest and around a
# single assign (which renames the window), a second-role assign of the same
# seat (rename skipped), a three-line assign --batch and a lineage add, then the
# governance log and each seat's window name.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-208-XXXXXX)
SERVER="pij-208-$$"
PRIME=pij-scratch-prime
# This shell's own seat identity must not leak into the private daemon.
unset TMUX TMUX_PANE PIJ_ID PIJ_SESSION_ID PIJ_PARENT_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID
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
register() { # <seat> <pane>
  curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" \
    -d "{\"id\":\"$1\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$2\"}" -H 'content-type: application/json' \
    "http://$PIJ_RS_ADDR/v1/register" | head -c 120; echo
}
# The scratch seats bind to the private server's panes (nothing is typed into them).
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
register "$PRIME" "$PANE"
export TMUX_PANE=$PANE
for n in 1 2 3 4; do
  tmux -L "$SERVER" new-window -d -t bench -n "w$n" 'sleep 100000'
  register "pij-scratch-h$n" "$(tmux -L "$SERVER" display-message -p -t "bench:w$n" '#{pane_id}')"
done

# Scratch repo: main plus a standing prime-governance worktree.
M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government/community"
echo g >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm "government"

run "$BIN" project create fleet --repo "$M" --prime "$PRIME"
for role in delivery storage; do
  "$BIN" directory add "$role" --title "$role" --kind specialist --lifecycle long-lived --holders one \
    --rule "Owns $role." --packet p --why "scratch role" >/dev/null; echo "add $role exit=$?"
done
"$BIN" directory add coder --title Coder --kind general --lifecycle per-task --holders many \
  --rule "Writes code." --packet p --why "scratch role" >/dev/null; echo "add coder exit=$?"
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

run "$BIN" directory assign delivery pij-scratch-h1 --why "first holding, renames the window"
reading after-assign
run "$BIN" directory assign storage pij-scratch-h1 --why "second holding, rename skipped"
reading after-second-role-assign

printf '# three holdings, one commit\ncoder pij-scratch-h2\ncoder pij-scratch-h3\ncoder pij-scratch-h4\n' >"$S/batch.txt"
run "$BIN" --json directory assign --batch "$S/batch.txt" --why "batch"
reading after-batch-of-3

run "$BIN" directory lineage add delivery pij-closed-elder --from 2026-10-01 --to 2026-10-02T12:00:00+10:00 --why "history kept elsewhere"
reading after-lineage-add

say "git -C gov log"; git -C "$G" log --format='%h %s' -n 8
say "window names"; tmux -L "$SERVER" list-windows -t bench -F '#{window_index} #{window_name}'

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
