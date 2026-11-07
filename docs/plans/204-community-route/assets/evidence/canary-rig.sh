#!/bin/bash
# Plan 204 fresh-prime canary rig (AC3, bp-0003): a private daemon on a private
# port and tmux server, its own state dir and HOME, and a scratch toy project.
# Never the machine daemon, ~/.pij-rs or the live governance branch.
#
#   canary-rig.sh <pij-rs binary> <port> <pij-fleet checkout>
#
# Writes <scratch>/env.sh (source it to drive the rig) and prints its path.
# The canary seat is an OMP session in the private tmux server's `canary`
# window; an `operator` window is a registered seat that sends the brief.
set -euo pipefail
BIN=$1 PORT=$2 REPO=$(cd "$3" && pwd)
S=$(mktemp -d /tmp/pij-204-canary-XXXXXX)
SERVER="pij-204-canary-$$"
mkdir -p "$S/bin" "$S/state" "$S/daemon-home" "$S/ref"
cp "$BIN" "$S/bin/pij-rs"
# `pij` is this branch's pij-rs: the route's community verbs are pij-rs's own
# (plan 213 forwards them unchanged), so a wrapper is the same surface. It pins
# the private daemon itself: in the first run the canary called it from a tool
# kernel with no PIJ_* env, and that one (read-only, refused) call reached the
# machine daemon.
printf '#!/bin/sh\nexport PIJ_RS_STATE_DIR="%s/state" PIJ_RS_ADDR="127.0.0.1:%s"\nexec "%s/bin/pij-rs" "$@"\n' "$S" "$PORT" "$S" >"$S/bin/pij"
chmod +x "$S/bin/pij"

# What the canary may read: the skill and the API doc, with relative links kept.
mkdir -p "$S/ref/skills" "$S/ref/docs/how"
cp -R "$REPO/skills/pij" "$S/ref/skills/pij"
cp "$REPO/docs/how/pij-rs-api.md" "$S/ref/docs/how/"

# The scratch project: the daily-digest example's PRD, blueprint and workstream.
M="$S/digest" G="$S/digest-gov"
mkdir -p "$M/docs/project"
for kind in prds blueprints workstreams; do
  mkdir -p "$M/docs/project/$kind"
  cp "$REPO/skills/pij/references/community/examples/$kind/"001-daily-digest.dd.* "$M/docs/project/$kind/"
done
printf '# daily digest\n\nA scratch project for the plan 204 canary: a daily digest of decisions and open\nquestions. Its PRD, blueprint and workstream are under docs/project/.\n' >"$M/README.md"
(
  cd "$M"
  git init -q -b main
  git -c user.name=scratch -c user.email=scratch@example.invalid add .
  git -c user.name=scratch -c user.email=scratch@example.invalid -c commit.gpgsign=false commit -qm "daily digest: docs"
  c=$(git commit-tree "$(git mktree </dev/null)" -m governance)
  git update-ref refs/heads/prime-governance "$c"
  git worktree add -q "$G" prime-governance
  mkdir -p "$G/.harness/government"
  echo "# daily digest government" >"$G/.harness/government/README.md"
  git -C "$G" -c user.name=scratch -c user.email=scratch@example.invalid add .
  git -C "$G" -c user.name=scratch -c user.email=scratch@example.invalid -c commit.gpgsign=false commit -qm government
)

ENV="$S/env.sh"
cat >"$ENV" <<VARS
export S='$S' M='$M' G='$G' SERVER='$SERVER'
export PIJ_RS_STATE_DIR='$S/state' PIJ_RS_ADDR='127.0.0.1:$PORT' PIJ_TMUX_SERVER='$SERVER'
export PATH='$S/bin':"\$PATH"
unset TMUX TMUX_PANE PIJ_SESSION_ID PIJ_PARENT_ID PIJ_SPAWN_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID COPILOT_LOADER_PID COPILOT_CLI COPILOT_CLI_PATH COPILOT_CLI_BINARY_VERSION COPILOT_CLI_RESOLVED_DIST_DIR
VARS
# shellcheck disable=SC1090
. "$ENV"

HOME="$S/daemon-home" "$S/bin/pij-rs" daemon --bind "$PIJ_RS_ADDR" --state-dir "$PIJ_RS_STATE_DIR" >>"$S/daemon.log" 2>&1 &
echo "export DAEMON_PID=$!" >>"$ENV"
for _ in $(seq 1 300); do
  curl -s "http://$PIJ_RS_ADDR/health" >/dev/null 2>&1 && [ -s "$PIJ_RS_STATE_DIR/daemon.key" ] && break
  sleep 0.2
done

# The private tmux server carries the private daemon's address to every pane.
tmux -L "$SERVER" -f /dev/null new-session -d -s canary -n operator -c "$S" 'sleep 100000'
for var in PIJ_RS_STATE_DIR PIJ_RS_ADDR PIJ_TMUX_SERVER PATH; do
  tmux -L "$SERVER" set-environment -g "$var" "${!var}"
done
# The driving seat's own identity and spawn correlation must not reach the
# canary: a Copilot canary refuses native registration when it inherits the
# parent's PIJ_SPAWN_ID or Copilot loader variables.
for var in TMUX_PANE PIJ_SESSION_ID PIJ_PARENT_ID PIJ_SPAWN_ID HARNESS_SESSION_ID CLAUDE_CODE_SESSION_ID COPILOT_AGENT_SESSION_ID CODEX_THREAD_ID COPILOT_LOADER_PID COPILOT_CLI COPILOT_CLI_PATH COPILOT_CLI_BINARY_VERSION COPILOT_CLI_RESOLVED_DIST_DIR; do
  tmux -L "$SERVER" set-environment -gu "$var"
done
OPERATOR=$(tmux -L "$SERVER" display-message -p -t canary:operator '#{pane_id}')
curl -s -H "Authorization: Bearer $(cat "$PIJ_RS_STATE_DIR/daemon.key")" -H 'content-type: application/json' \
  -d "{\"id\":\"pij-canary-operator\",\"harness\":\"omp\",\"folder\":\"$S\",\"pane\":\"$OPERATOR\"}" \
  "http://$PIJ_RS_ADDR/v1/register" >"$S/operator-register.json"
echo "export OPERATOR_PANE='$OPERATOR'" >>"$ENV"
TMUX_PANE="$OPERATOR" "$S/bin/pij-rs" project create digest --repo "$M" >"$S/project-create.txt" 2>&1
echo "$ENV"
