#!/bin/bash
# Plan 203 private-daemon evidence (bp-0006, AC6): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never the real state dir, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest, then around
# ten `pij directory` views of a project with holders, a memory note, an
# output, roadmaps and blueprints, so the columns, prompts and reviews are
# computed. The view must launch nothing.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-203-XXXXXX)
SERVER="pij-203-$$"
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
for role in reviewer:many qa:one; do
  run "$BIN" directory add "${role%%:*}" --title "${role%%:*}" --kind general --lifecycle long-lived \
    --holders "${role##*:}" --rule "r" --packet "p" --why "setup"
done
run "$BIN" directory assign reviewer "$HOLDER" --why "reviews"
run "$BIN" directory assign qa "$HOLDER" --why "a second domain"
run "$BIN" directory memory add reviewer --title "What broke" --hook "h" --body "b"
TMUX_PANE=$HOLDER_PANE "$BIN" --json output verdict --role reviewer --subject pr:200 --head "$HEAD" \
  --result pass --body "clean" --project fleet | notified
mkdir -p "$M/docs/project/roadmaps" "$M/docs/project/blueprints"
cat >"$M/docs/project/blueprints/001-x.dd.json" <<'JSON'
{"sections": [{"name": "components", "value": [{"id": "c-core", "where": "crates/engine"}]}]}
JSON
B='"../blueprints/001-x.dd.json#components/c-core"'
cat >"$M/docs/project/roadmaps/001-x.dd.json" <<JSON
{"sections": [{"name": "steps", "value": [
  {"id": "s01", "state": "complete", "builds": [$B]},
  {"id": "s02", "state": "complete", "builds": [$B]},
  {"id": "s03", "state": "planned", "needs": ["role:docs-system"], "builds": [$B]}
]}]}
JSON
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

say "ten views (--json)"
for _ in $(seq 1 10); do "$BIN" --json directory --project fleet >/dev/null; done
reading after-ten-views

run "$BIN" directory --project fleet
reading after-human-view

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
