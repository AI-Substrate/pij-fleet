#!/bin/bash
# Plan 201 private-daemon evidence (bp-0004, AC4): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest, around two
# memory adds at once, the memory reads, a close of a seat holding no role and
# a close of a role holder (its memory turn, bounded at 5 s here).
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-201-XXXXXX)
SERVER="pij-201-$$"
SEAT=pij-scratch-marten HOLDER=pij-scratch-holder PLAIN=pij-scratch-plain
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
key() { cat "$PIJ_RS_STATE_DIR/daemon.key"; }
register() { # <id> <pane> [parent]
  local parent=${3:+,\"parent\":\"$3\"}
  curl -s -H "Authorization: Bearer $(key)" -H 'content-type: application/json' \
    -d "{\"id\":\"$1\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$2\"$parent}" \
    "http://$PIJ_RS_ADDR/v1/register" | head -c 100; echo
}

tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
PIJ_RS_MEMORY_TURN_SECS=5 "$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done
echo "private daemon pid=$PID port=$PORT state=$PIJ_RS_STATE_DIR tmux=-L $SERVER bound=5s"
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
register "$SEAT" "$PANE"
export TMUX_PANE=$PANE
for name in w1 w2; do tmux -L "$SERVER" new-window -d -t bench -n "$name" 'sleep 100000'; done
HOLDER_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}')
PLAIN_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w2 '#{pane_id}')
register "$HOLDER" "$HOLDER_PANE" "$SEAT"
register "$PLAIN" "$PLAIN_PANE" "$SEAT"

M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo g >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm government

run "$BIN" project create fleet --repo "$M" --prime "$SEAT"
run "$BIN" directory add reviewer --title reviewer --kind general --lifecycle long-lived --holders many \
  --rule "r" --packet "p" --why "setup"
run "$BIN" directory assign reviewer/114 "$HOLDER" --why "review #114"
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

say "two memory adds at once (one from stdin)"
printf 'what broke, and the fix\n' | "$BIN" directory memory add reviewer --title "PR 200: what broke" \
  --hook "open before reviewing a writer change" --tags review,writer --body-file - &
FIRST=$!
"$BIN" directory memory add reviewer --title "Cost receipts" --hook "how to read ping spawns" \
  --tags cost --body "two readings a known number of seconds apart" &
wait "$FIRST" "$!"  # not the daemon, which is also a background job
reading after-two-adds

run "$BIN" directory memory reviewer
reading after-index
run "$BIN" directory memory reviewer --note pr-200-what-broke
reading after-note
run "$BIN" directory memory delete reviewer
run "$BIN" directory memory add reviewer --title "PR 200: what broke" --hook h --body b
reading after-refusals  # the duplicate resolves the governance root (3 git) before refusing

run "$BIN" close "$PLAIN" --reason "done"
reading after-close-non-holder

run "$BIN" close "$HOLDER" --reason "done"
reading after-close-holder

say "git -C gov log"; git -C "$G" log --format='%h %s' -n 6
say "files in each memory commit"
for commit in $(git -C "$G" log --format=%h --grep '^community: memory' -n 2); do
  git -C "$G" show --name-only --format='%h %s' "$commit"
done
say "memory/INDEX.md at HEAD"; git -C "$G" show HEAD:.harness/government/community/roles/reviewer/memory/INDEX.md
say "memory.turn on the spine"
"$BIN" --json spine events --peer "$HOLDER" | python3 -c "
import json, sys
for e in json.load(sys.stdin)['data']['events']:
    if e['kind'] == 'memory.turn': print(e['kind'], e['payload'])"

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
