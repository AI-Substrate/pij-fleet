#!/bin/bash
# Plan 196 private-daemon evidence (bp-0004, bp-0006): temp HOME and state dir,
# a private -L tmux server, a private port, a scratch main checkout and a fake
# gh (PIJ_GH). Never the machine daemon, never ~/.pij-rs, never a real PR.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints ping readings (pid, spawn counts) at rest and around one claim, one
# claims list, one delivered (and its re-run), the PM's card, and a close
# that releases the closed seat's claims.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-196-XXXXXX)
SERVER="pij-196-$$"
SEAT=pij-scratch-wildcat PM=pij-scratch-pm
unset TMUX TMUX_PANE PIJ_SESSION_ID PIJ_PARENT_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID
export HOME="$S/home" PIJ_RS_STATE_DIR="$S/state" PIJ_TMUX_SERVER="$SERVER" PIJ_RS_ADDR="127.0.0.1:$PORT" PIJ_GH="$S/bin/gh"
mkdir -p "$HOME" "$PIJ_RS_STATE_DIR" "$S/bin"
git config --global user.name scratch
git config --global user.email scratch@example.invalid
git config --global commit.gpgsign false
git config --global init.defaultBranch main

# The fake gh: PR 7 merged, delivering one requirement and one step, touching
# plan 191's folder and migration 0034.
cat >"$PIJ_GH" <<'GH'
#!/bin/sh
cat <<'JSON'
{"number":7,"state":"MERGED","mergeCommit":{"oid":"1111111111111111111111111111111111111111"},"url":"https://example.invalid/pr/7",
 "body":"Delivers: docs/project/prds/001-demo.dd.json#requirements/req-01, docs/project/roadmaps/001-demo.dd.json#steps/s01\n",
 "files":[{"path":"docs/plans/191-demo/plan.dd.json"},{"path":"crates/store/migrations/0034_demo.sql"}]}
JSON
GH
chmod +x "$PIJ_GH"

say() { printf '\n$ %s\n' "$*"; }
run() { say "$*"; "$@"; echo "exit=$?"; }
reading() { # <label>
  "$BIN" --json ping | python3 -c "
import json, sys
d = json.load(sys.stdin)['data']
s = d['spawns']['by_op']
print('reading $1: pid=%s gh=%s total=%s by_op=%s' % (d['pid'], d['gh'], d['spawns']['total'], json.dumps(s, sort_keys=True)))"
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
    "http://$PIJ_RS_ADDR/v1/register" | head -c 100; echo
}
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
register "$SEAT" "$PANE"
tmux -L "$SERVER" new-window -d -t bench -n w1 'sleep 100000'
register "$PM" "$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}')"
export TMUX_PANE=$PANE

# Scratch main checkout: plans up to 190, migrations up to 0033, a PRD and a roadmap.
M="$S/main"
mkdir -p "$M/docs/plans/190-a" "$M/crates/store/migrations" "$M/docs/project/prds" "$M/docs/project/roadmaps"
touch "$M/docs/plans/190-a/plan.dd.json" "$M/crates/store/migrations/0033_x.sql"
echo '{"sections":[{"name":"requirements","value":[{"id":"req-01","delivered_by":"#3"}]}]}' >"$M/docs/project/prds/001-demo.dd.json"
echo '{"sections":[{"name":"steps","value":[{"id":"s01"}]}]}' >"$M/docs/project/roadmaps/001-demo.dd.json"
git -C "$M" init -q && git -C "$M" add . && git -C "$M" commit -qm main
cd "$M" || exit 1

run "$BIN" project create demo --repo "$M" --prime "$SEAT"
sleep 10  # let the pane observer finish attaching to the new seats' panes
reading rest-1; sleep 60; reading rest-2

run "$BIN" claim plan --for "demo plan"
reading after-claim
run "$BIN" claims
reading after-claims
run "$BIN" claim migration --for "demo migration"
run "$BIN" claim prd --for "held by the closing seat"
reading before-delivered
run "$BIN" delivered 7 --pm "$PM"
reading after-delivered
run "$BIN" delivered 7 --pm "$PM"
reading after-rerun
say "PM card"; "$BIN" --json state "$PM" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['data']['deliveries']))"
run "$BIN" close "$SEAT" --reason done
TMUX_PANE=$(tmux -L "$SERVER" display-message -p -t bench:w1 '#{pane_id}') run "$BIN" claims
say "git -C main status --porcelain (empty: no project file changed)"; git -C "$M" status --porcelain; echo "(end)"

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
