#!/bin/sh
# PROC-fleet-0001 step 0002: relaunch a Copilot seat in its own pane on a new reasoning effort, resuming its session,
# with its PIJ_* (but not PIJ_SPAWN_ID) and COPILOT_* environment. Prints variable names, never their values.
set -eu
seat=${PROC_IN_SEAT:?}
effort=${PROC_IN_EFFORT:?}
row=$(pij list --json | jq -c --arg s "$seat" '.data.seats[] | select(.id == $s)')
[ -n "$row" ] || { echo "no seat $seat in pij list" >&2; exit 1; }
field() { printf %s "$row" | jq -r "$1"; }
pane=$(field .pane)
session=$(field .session)
folder=$(field .folder)
pid=$(field .proc.pid)
for value in "$pane" "$session" "$folder" "$pid"; do
	if [ -z "$value" ] || [ "$value" = null ]; then
		echo "$seat's row lacks a pane, session, folder or process" >&2
		exit 1
	fi
done
flags=$(/bin/ps -o command= -p "$pid" | cut -d' ' -f2- | sed -E 's/ --(reasoning-)?effort[= ][^ ]+//; s/ --resume[= ][^ ]+//')
envs=$(/bin/ps -Eww -o command= -p "$pid" | tr ' ' '\n' | grep -E '^(PIJ_|COPILOT_)' | grep -v '^PIJ_SPAWN_ID=' | tr '\n' ' ')
launcher=$(command -v copilot)
echo "relaunching $seat in $pane on --effort $effort, resuming $session"
echo "carrying: $(printf '%s\n' $envs | sed 's/=.*//' | tr '\n' ' ')"
# respawn-pane -k ends only this pane's processes (never kill by name: AGENTS.md, D-C34-3).
tmux respawn-pane -k -t "$pane" -c "$folder" "env $envs $launcher $flags --effort $effort --resume=$session"
# Wait, at most about a minute, for the seat to register on its new process.
tries=0
while [ "$tries" -lt 30 ]; do
	new=$(pij list --json | jq -r --arg s "$seat" '.data.seats[] | select(.id == $s) | .proc.pid')
	if [ -n "$new" ] && [ "$new" != null ] && [ "$new" != "$pid" ]; then
		echo "registered on pid $new"
		exit 0
	fi
	/bin/sleep 2
	tries=$((tries + 1))
done
echo "$seat did not register a new process within a minute; look at its pane" >&2
exit 1
