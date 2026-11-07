#!/bin/bash
# Plan 200 private-daemon evidence (bp-0005, AC5): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never the real state dir, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest and around a
# role send, a verdict that answers it (one writer commit, one notification),
# an unrequested note (an FYI to the writer's parent), two outputs at once,
# and the outputs reads.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-200-XXXXXX)
SERVER="pij-200-$$"
SEAT=pij-scratch-marten HOLDER=pij-scratch-holder ASKER=pij-scratch-asker
HEAD=0123456789abcdef0123456789abcdef01234567
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
notified() { python3 -c "
import json, sys
d = json.load(sys.stdin)
print('ok=%s notified=%s path=%s' % (d.get('ok'), json.dumps(d['data'].get('notified')), d['data'].get('path')))"; }

tmux -L "$SERVER" -f /dev/null new-session -d -s bench -n w0 'sleep 100000'
"$BIN" daemon --bind "127.0.0.1:$PORT" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done
echo "private daemon pid=$PID port=$PORT state=$PIJ_RS_STATE_DIR tmux=-L $SERVER"
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
register "$SEAT" "$PANE"
export TMUX_PANE=$PANE
for name in w1 w2; do tmux -L "$SERVER" new-window -d -t bench -n "$name" 'sleep 100000'; done
HOLDER_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}')
ASKER_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w2 '#{pane_id}')
register "$HOLDER" "$HOLDER_PANE" "$SEAT"
register "$ASKER" "$ASKER_PANE" "$SEAT"

M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo g >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm government

run "$BIN" project create fleet --repo "$M" --prime "$SEAT"
run "$BIN" directory add reviewer --title reviewer --kind general --lifecycle long-lived --holders many \
  --rule "r" --packet "p" --why "setup"
run "$BIN" directory assign reviewer "$HOLDER" --why "reviews"
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

say "the asker sends to role:reviewer (reply wake)"
REQ=$(TMUX_PANE=$ASKER_PANE "$BIN" --json send --to role:reviewer --body "review pr:200 at $HEAD" --project fleet \
  | python3 -c "import json,sys; d=json.load(sys.stdin)['data']; print(d['request']['id'])")
echo "request=$REQ"
reading after-role-send

say "the holder answers with a verdict (--answers the request)"
TMUX_PANE=$HOLDER_PANE "$BIN" --json output verdict --role reviewer --subject pr:200 --head "$HEAD" \
  --result pass --answers "$REQ" --body "clean" --project fleet | notified
reading after-answered-verdict

say "an unrequested note (an FYI to the writer's parent)"
TMUX_PANE=$HOLDER_PANE "$BIN" --json output note --role reviewer --subject pr:200 \
  --body "a note nobody asked for" --project fleet | notified
reading after-unrequested-note

say "two outputs at once"
TMUX_PANE=$HOLDER_PANE "$BIN" --json output note --role reviewer --subject pr:201 --body one --project fleet >"$S/one.json" &
FIRST=$!
TMUX_PANE=$HOLDER_PANE "$BIN" --json output note --role reviewer --subject pr:201 --body two --project fleet >"$S/two.json" &
wait "$FIRST" "$!"  # not the daemon, which is also a background job
notified <"$S/one.json"; notified <"$S/two.json"
reading after-two-at-once

run "$BIN" outputs --subject pr:200 --head "$HEAD" --project fleet
run "$BIN" outputs --role reviewer --project fleet
reading after-reads

say "git -C gov log"; git -C "$G" log --format='%h %s' -n 6
say "files in each output commit"
for commit in $(git -C "$G" log --format=%h -E --grep '^community: reviewer (verdict|note)' -n 4); do
  git -C "$G" show --name-only --format='%h %s' "$commit"
done

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
