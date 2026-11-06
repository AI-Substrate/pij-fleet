#!/bin/bash
# Plan 202 private-daemon evidence (bp-0004, AC4): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest and around a
# role add (which rewrites a hand-written ROUTES.md), an overlapping add,
# `directory routes`, `routes --write` (current, then stale), an assign and a
# release, then the held FYIs and the governance log.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-202-XXXXXX)
SERVER="pij-202-$$"
SEAT=pij-scratch-prime HOLDER=pij-scratch-holder
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
register "$SEAT" "$PANE"
export TMUX_PANE=$PANE
tmux -L "$SERVER" new-window -d -t bench -n w1 'sleep 100000'
register "$HOLDER" "$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}')"

# Scratch repo: main plus a standing prime-governance worktree with a
# hand-written ROUTES.md, as the live branch has.
M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government/community"
echo g >"$G/.harness/government/README.md"
printf '# Routing: send it, don'"'"'t do it\n\n| What | Send to |\n|---|---|\n| merges | prime |\n' >"$G/.harness/government/community/ROUTES.md"
git -C "$G" add . && git -C "$G" commit -qm "government, hand-written routes"

run "$BIN" project create fleet --repo "$M" --prime "$SEAT"
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

run "$BIN" directory add delivery --title Delivery --kind specialist --lifecycle long-lived --holders one \
  --scope-path 'crates/delivery/**' --rule "Owns the send path." --packet "p" --why "first role"
reading after-add
run "$BIN" --json directory add storage --title Storage --kind specialist --lifecycle long-lived --holders one \
  --scope-path 'crates/delivery/queue.rs' --rule "Owns the queue." --packet "p" --why "overlapping role"
reading after-overlapping-add
say "ROUTES.md at HEAD"; git -C "$G" show HEAD:.harness/government/community/ROUTES.md

run "$BIN" directory routes
reading after-routes
run "$BIN" directory routes --write --why "check"
reading after-routes-write-current

say "a hand edit makes ROUTES.md stale"
echo "edited by hand" >"$G/.harness/government/community/ROUTES.md" && git -C "$G" commit -qam "a hand edit"
run "$BIN" --json directory routes --write --why "adopt"
reading after-routes-write-stale

run "$BIN" directory assign delivery "$HOLDER" --why "own it"
reading after-assign
run "$BIN" directory release delivery "$HOLDER" --why "rotate"
reading after-release

say "held FYIs (recipient, sender, first line)"
DB=$(find "$PIJ_RS_STATE_DIR" -maxdepth 1 -type f -name '*.sqlite*' ! -name '*-wal' ! -name '*-shm' | head -1)
sqlite3 "$DB" "SELECT recipient, sender, substr(body, 1, instr(body, char(10)) - 1) FROM fyis ORDER BY held_at_ms, id"
say "git -C gov log"; git -C "$G" log --format='%h %s' -n 8
say "files in the first role commit"; git -C "$G" show --name-only --format= "$(git -C "$G" log --format=%h --grep='add role delivery')"

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
