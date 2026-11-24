#!/bin/sh
# PROC-fleet-0001: relaunch a Copilot seat in its own pane on a new reasoning effort, resuming its session.
#   relaunch.sh relaunch    step s0002
#   relaunch.sh effort-is   its done check: the seat's process runs on exactly PROC_IN_EFFORT
# The seat keeps its PIJ_* (not PIJ_SPAWN_ID) and COPILOT_* environment, except any name that looks like a secret.
# Values never go on a command line (req-13): they go through a 0600 file the pane's shell reads, then deletes.
# Every word the pane runs is quoted. It prints variable names, never their values.
set -euf # -f: words from ps are split on spaces but never globbed
ps_bin=${RELAUNCH_PS:-/bin/ps} # tests point this at a stub
seat=${PROC_IN_SEAT:?}
effort=${PROC_IN_EFFORT:?}
case $effort in
none | minimal | low | medium | high | xhigh | max) ;;
*)
	echo "effort must be one of none, minimal, low, medium, high, xhigh, max (copilot --help), not '$effort'" >&2
	exit 2
	;;
esac

row=$(pij list --json | jq -c --arg s "$seat" '.data.seats[] | select(.id == $s)')
[ -n "$row" ] || {
	echo "no seat $seat in pij list" >&2
	exit 1
}
field() { printf %s "$row" | jq -r "$1"; }
pid=$(field .proc.pid)
case $pid in '' | null | *[!0-9]*)
	echo "$seat's row lacks a process" >&2
	exit 1
	;;
esac

if [ "${1:-}" = effort-is ]; then
	# Exact words, never a pattern: `--effort <e>`, `--effort=<e>`, or the same with --reasoning-effort.
	"$ps_bin" -o command= -p "$pid" | tr ' ' '\n' | awk -v e="$effort" '
		(prev == "--effort" || prev == "--reasoning-effort") && $0 == e { found = 1 }
		$0 == "--effort=" e || $0 == "--reasoning-effort=" e { found = 1 }
		{ prev = $0 }
		END { exit !found }'
	exit
fi

pane=$(field .pane)
session=$(field .session)
folder=$(field .folder)
for value in "$pane" "$session" "$folder"; do
	if [ -z "$value" ] || [ "$value" = null ]; then
		echo "$seat's row lacks a pane, session or folder" >&2
		exit 1
	fi
done
launcher=$(command -v copilot)

# One word, single-quoted for sh: ' becomes '\''.
quote() { printf "'%s'" "$(printf %s "$1" | sed "s/'/'\\\\''/g")"; }

# The seat's own flags, minus its effort and resume. ps joins argv with spaces, so an argument holding a space comes
# back split (a known limit of this procedure).
words=
skip=
for word in $("$ps_bin" -o command= -p "$pid" | cut -d' ' -f2-); do
	if [ -n "$skip" ]; then
		skip=
		continue
	fi
	case $word in
	--effort | --reasoning-effort | --resume) skip=1 ;;
	--effort=* | --reasoning-effort=* | --resume=*) ;;
	*) words="$words $(quote "$word")" ;;
	esac
done

umask 077
envfile=$(mktemp "${TMPDIR:-/tmp}/proc-fleet-0001.XXXXXX")
trap 'rm -f "$envfile"' EXIT
carried=
skipped=
for pair in $("$ps_bin" -Eww -o command= -p "$pid" | tr ' ' '\n' | grep -E '^(PIJ|COPILOT)_[A-Z0-9_]*=' || true); do
	name=${pair%%=*}
	case $name in
	PIJ_SPAWN_ID | *KEY* | *TOKEN* | *SECRET* | *PASS* | *CREDENTIAL* | *AUTH*)
		skipped="$skipped $name"
		continue
		;;
	esac
	printf 'export %s=%s\n' "$name" "$(quote "${pair#*=}")" >>"$envfile"
	carried="$carried $name"
done

echo "relaunching $seat in $pane on --effort $effort, resuming $session"
echo "carrying:$carried"
[ -z "$skipped" ] || echo "not carrying (spawn id or secret-like):$skipped"
command=". $(quote "$envfile"); rm -f $(quote "$envfile"); exec $(quote "$launcher")$words --effort $(quote "$effort") $(quote "--resume=$session")"
# respawn-pane -k ends only this pane's processes (never kill by name: AGENTS.md, D-C34-3). The pane deletes the file.
tmux respawn-pane -k -t "$pane" -c "$folder" "$command"
trap - EXIT

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
