#!/bin/bash
# Plan 194 private-daemon evidence (bp-0006, AC7): temp HOME and state dir, a
# private -L tmux server, a private port, scratch repos. Never the machine
# daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port>
#
# Prints a transcript: ping readings (pid, spawn counts) at rest and around
# `directory templates`, the view with its prompts, one `add --from template`,
# and `template-diff`.
set -uo pipefail
BIN=$1 PORT=$2
S=$(mktemp -d /tmp/pij-194-XXXXXX)
SERVER="pij-194-$$"
SEAT=pij-scratch-marsupial
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
echo "private daemon pid=$PID port=$PORT"
# The scratch seat binds to the private server's pane (nothing is typed into it).
PANE=$(tmux -L "$SERVER" display-message -p -t bench:w0 '#{pane_id}')
curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" \
  -d "{\"id\":\"$SEAT\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$PANE\"}" -H 'content-type: application/json' \
  "http://$PIJ_RS_ADDR/v1/register" | head -c 120; echo
export TMUX_PANE=$PANE

# Scratch repo: main plus a standing prime-governance worktree.
M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo g >"$G/.harness/government/README.md"
git -C "$G" add . && git -C "$G" commit -qm government

run "$BIN" project create fleet --repo "$M" --prime "$SEAT"
sleep 10  # let the pane observer settle
reading rest-1; sleep 30; reading rest-2

run "$BIN" directory templates
reading after-templates

run "$BIN" directory
reading after-view

run "$BIN" directory add security --from template:security --why "recommended role from the security template"
reading after-add-from-template

run "$BIN" directory template-diff security
reading after-template-diff

run "$BIN" directory
reading after-view-2

say "git -C gov log"; git -C "$G" log --format='%h %s' -n 3
say "security meta at HEAD"; git -C "$G" show HEAD:.harness/government/community/roles/security/role.dd.json | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['sections'][0]['value'], indent=1))"

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
