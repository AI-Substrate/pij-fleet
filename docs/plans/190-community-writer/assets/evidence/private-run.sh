#!/bin/bash
# Plan 190 private-daemon evidence (bp-0003, bp-0005, bp-0007): temp HOME and
# state dir, a private -L tmux server, a private port, scratch repos. Never the
# machine daemon, never ~/.pij-rs, never the live governance branch.
#
#   private-run.sh <pij-rs binary> <port> <batch fixtures dir>
#
# Prints a transcript: ping readings (pid, ddocs, spawn counts) at rest and
# around one `directory add` and one batch, the refusals, and the commits.
set -uo pipefail
BIN=$1 PORT=$2 FIX=$3
S=$(mktemp -d /tmp/pij-190-XXXXXX)
SERVER="pij-190-$$"
SEAT=pij-scratch-marten
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

# Scratch repo: main plus a standing prime-governance worktree with an
# unstaged edit, a staged edit and an untracked file (AC4's shape).
M="$S/main" G="$S/gov"
mkdir -p "$M" && cd "$M" && git init -q && echo main >README.md && git add README.md && git commit -qm main
c=$(git commit-tree "$(git mktree </dev/null)" -m governance) && git update-ref refs/heads/prime-governance "$c"
git worktree add -q "$G" prime-governance
mkdir -p "$G/.harness/government" && echo g >"$G/.harness/government/README.md"
echo a >"$G/a.md" && echo b >"$G/b.md" && git -C "$G" add . && git -C "$G" commit -qm government
echo "prime's unstaged" >>"$G/a.md"; echo "prime's staged" >>"$G/b.md"; git -C "$G" add b.md; echo new >"$G/c.md"
say "git -C gov status --short (before)"; git -C "$G" status --short

run "$BIN" project create fleet --repo "$M"
sleep 10  # let the pane observer finish attaching to the new seat's pane
reading rest-1; sleep 60; reading rest-2

run "$BIN" directory add auditor --title Auditor --kind specialist --lifecycle long-lived --holders one \
  --default-limit 45m --rule "A verdict on every PR at its exact head." --harness copilot --effort high \
  --output-kind verdict --packet "You hold the security role." --why "first role through the writer"
reading after-add

# All or nothing: the same eight with one document missing its scope.
SPOILED="$S/spoiled" && mkdir -p "$SPOILED" && cp "$FIX"/*.dd.json "$SPOILED"/
python3 - "$SPOILED/reviewer.dd.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["sections"] = [x for x in d["sections"] if x["name"] != "scope"]
json.dump(d, open(sys.argv[1], "w"), indent="\t")
PY
run "$BIN" directory add --batch "$SPOILED" --why "one bad document"
reading after-spoiled-batch
say "git -C gov status --short (after the refused batch)"; git -C "$G" status --short

run "$BIN" directory add --batch "$FIX" --why "import eight roles shaped like the hand-written ones"
reading after-batch

say "refusals"
run "$BIN" directory add auditor --rule x --packet y --why again
run "$BIN" directory add qa --title QA --kind general --lifecycle per-task --holders many --packet p --why "no scope"
mkdir -p "$G/.harness/government/community/roles/pm-002"
echo foreign >"$G/.harness/government/community/roles/pm-002/role.dd.json"
run "$BIN" directory add pm-002 --rule x --packet y --why foreign
mkdir -p "$S/bare" && git -C "$S/bare" init -q
run "$BIN" project create bare --repo "$S/bare"
run "$BIN" directory add qa --project bare --rule x --packet y --why "no governance"
reading after-refusals

say "git -C gov status --short (after)"; git -C "$G" status --short
say "git -C gov log --stat"; git -C "$G" log --format='%h %s%n%b' --stat -n 3
say "the auditor role as committed"; git -C "$G" show HEAD~1:.harness/government/community/roles/auditor/role.dd.json
say "ddocs validate on every committed role"
for f in $(git -C "$G" ls-files '*/role.dd.json'); do
  printf '%s ' "$f"; (cd "$G" && ddocs --json validate "$f" --depth 0 | python3 -c "import json,sys;print(json.load(sys.stdin)['status'])")
done

kill -TERM "$PID"; for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
tmux -L "$SERVER" kill-server 2>/dev/null
rm -rf "$S"
