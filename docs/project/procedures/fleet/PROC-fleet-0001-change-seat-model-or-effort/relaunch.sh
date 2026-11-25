#!/bin/sh
# PROC-fleet-0001: relaunch a Copilot seat in its own pane on a new model, effort or both, resuming its session.
#   relaunch.sh relaunch    step s0002
#   relaunch.sh flags-are   its done check: the seat's process runs on exactly PROC_IN_MODEL and PROC_IN_EFFORT, with
#                           the cache-proxy environment the model needs
# The seat keeps its PIJ_* (not PIJ_SPAWN_ID) and COPILOT_* environment, except any name that looks like a secret.
# The cache-proxy rule (crates/daemon/src/cache_proxy.rs): only a Claude model goes through the proxy. A new model that
# is not Claude runs plain Copilot, so its COPILOT_PROVIDER_* are dropped; a Claude model on a proxied seat keeps them,
# with the model id and prompt limit the proxy lists for it.
# Values never go on a command line (req-13): they go through a 0600 file the pane's shell reads, then deletes.
# Every word the pane runs is quoted. It prints variable names, never their values.
set -euf # -f: words from ps are split on spaces but never globbed
ps_bin=${RELAUNCH_PS:-/bin/ps}     # tests point these at stubs
pij_rs=${RELAUNCH_PIJ_RS:-pij-rs}
seat=${PROC_IN_SEAT:?}
effort=${PROC_IN_EFFORT:-}
model=${PROC_IN_MODEL:-}
[ -n "$effort$model" ] || {
	echo "give a model, an effort or both (--arg model=<id>, --arg effort=<effort>)" >&2
	exit 2
}
case $effort in
'' | none | minimal | low | medium | high | xhigh | max) ;;
*)
	echo "effort must be one of none, minimal, low, medium, high, xhigh, max (copilot --help), not '$effort'" >&2
	exit 2
	;;
esac
# A model id in the shape pij accepts for one, without brackets.
case $model in
[!A-Za-z0-9]* | *[!A-Za-z0-9._-]*)
	echo "model must be a model id (letters, digits, '.', '_' and '-', starting with a letter or digit), not '$model'" >&2
	exit 2
	;;
esac
[ "${#model}" -le 80 ] || {
	echo "model id is longer than 80 characters" >&2
	exit 2
}
fusion=
claude=
case $model in hydrafusion | hydrafusion-*) fusion=1 ;; claude*) claude=1 ;; esac
if [ -n "$fusion" ] && [ -n "$effort" ]; then
	echo "HydraFusion routes effort itself; leave the effort out (Copilot refuses one for $model)" >&2
	exit 2
fi
base=${model%-1m} # the proxy serves a `-1m` model as its base id

row=$(pij list --json | jq -c --arg s "$seat" '.data.seats[] | select(.id == $s)')
[ -n "$row" ] || {
	echo "no seat $seat in pij list" >&2
	exit 1
}
field() { printf %s "$row" | jq -r "$1"; }
seat_pid=$(field .proc.pid)
pane=$(field .pane)
case $seat_pid in '' | null | *[!0-9]*)
	echo "$seat's row lacks a process" >&2
	exit 1
	;;
esac
case $pane in '' | null)
	echo "$seat's row lacks a pane" >&2
	exit 1
	;;
esac
# Read the launch from the pane's own process, never from pij's recorded pid: after a Copilot /restart the recorded
# process is the loader's child, whose argv holds no --model or --effort and whose environment holds the loader's own.
pid=$(tmux display-message -p -t "$pane" '#{pane_pid}')
case $pid in '' | *[!0-9]*)
	echo "tmux names no process for $seat's pane $pane" >&2
	exit 1
	;;
esac

# Exact words, never a pattern: `<flag> <v>` or `<flag>=<v>`, for any of the flag spellings given.
has_flag() {
	value=$1
	shift
	"$ps_bin" -o command= -p "$pid" | tr ' ' '\n' | awk -v v="$value" -v names="$*" '
		BEGIN { n = split(names, f, " "); for (i = 1; i <= n; i++) flag[f[i]] = 1 }
		(prev in flag) && $0 == v { found = 1 }
		{ for (k in flag) if ($0 == k "=" v) found = 1; prev = $0 }
		END { exit !found }'
}
provider_words() { "$ps_bin" -Eww -o command= -p "$pid" | tr ' ' '\n' | grep -E '^COPILOT_PROVIDER_[A-Z0-9_]*=' || true; }

if [ "${1:-}" = flags-are ]; then
	[ -z "$effort" ] || has_flag "$effort" --effort --reasoning-effort || exit 1
	if [ -n "$model" ]; then
		has_flag "$model" --model || exit 1
		if [ -n "$fusion" ]; then
			"$ps_bin" -o command= -p "$pid" | tr ' ' '\n' | grep -Eq '^--(reasoning-)?effort(=|$)' && exit 1
		fi
		provider=$(provider_words)
		if [ -z "$claude" ]; then
			[ -z "$provider" ] || exit 1 # not Claude: plain Copilot, no provider environment
		elif [ -n "$provider" ]; then
			printf '%s\n' "$provider" | grep -Fqx "COPILOT_PROVIDER_MODEL_ID=$base" || exit 1
		fi
	fi
	exit 0
fi

session=$(field .session)
folder=$(field .folder)
for value in "$session" "$folder"; do
	if [ -z "$value" ] || [ "$value" = null ]; then
		echo "$seat's row lacks a session or folder" >&2
		exit 1
	fi
done
launcher=$(command -v copilot)

# One word, single-quoted for sh: ' becomes '\''.
quote() { printf "'%s'" "$(printf %s "$1" | sed "s/'/'\\\\''/g")"; }

# A Claude model on a proxied seat needs the proxy to serve it; ask before anything changes.
proxied=
provider_words | grep -q '^COPILOT_PROVIDER_TYPE=' && proxied=1
limit=
if [ -n "$claude" ] && [ -n "$proxied" ]; then
	limit=$("$pij_rs" cache-proxy status --json | jq -r --arg m "$base" '
		.data.status | select(.ready == true and .state == "ready") | .models[$m].max_prompt_tokens // empty')
	case $limit in '' | *[!0-9]*)
		echo "the cache proxy is not ready or does not serve $base (pij-rs cache-proxy status); choose a Claude model it lists" >&2
		exit 1
		;;
	esac
fi

# The seat's own flags, minus the ones this run sets: the words after the copilot entry point (the pane may run it
# as `node <path>/copilot`). ps joins argv with spaces, so an argument holding a space comes back split (a known limit).
words=
skip=
started=
for word in $("$ps_bin" -o command= -p "$pid"); do
	if [ -z "$started" ]; then
		case $word in copilot | */copilot) started=1 ;; esac
		continue
	fi
	if [ -n "$skip" ]; then
		skip=
		continue
	fi
	case $word in
	--resume) skip=1 ;;
	--resume=*) ;;
	--effort | --reasoning-effort) [ -n "$effort$fusion" ] && skip=1 || words="$words $(quote "$word")" ;;
	--effort=* | --reasoning-effort=*) [ -n "$effort$fusion" ] || words="$words $(quote "$word")" ;;
	--model | --context) [ -n "$model" ] && skip=1 || words="$words $(quote "$word")" ;;
	--model=* | --context=*) [ -n "$model" ] || words="$words $(quote "$word")" ;;
	*) words="$words $(quote "$word")" ;;
	esac
done
[ -n "$started" ] || {
	echo "the process in $seat's pane $pane is not copilot" >&2
	exit 1
}
set_flags=
if [ -n "$model" ]; then
	# As pij's spawn does (crates/harnesses/src/spawn.rs): a fusion model needs the default context tier.
	tier=long_context
	[ -z "$fusion" ] || tier=default
	set_flags=" --model $(quote "$model") --context $tier"
fi
[ -z "$effort" ] || set_flags="$set_flags --effort $(quote "$effort")"

umask 077
envfile=$(mktemp "${TMPDIR:-/tmp}/proc-fleet-0001.XXXXXX")
trap 'rm -f "$envfile"' EXIT
carried=
skipped=
dropped=
for pair in $("$ps_bin" -Eww -o command= -p "$pid" | tr ' ' '\n' | grep -E '^(PIJ|COPILOT)_[A-Z0-9_]*=' || true); do
	name=${pair%%=*}
	value=${pair#*=}
	case $name in
	COPILOT_PROVIDER_MAX_PROMPT_TOKENS) ;; # a prompt limit, not a secret: an exact name, never a pattern
	PIJ_SPAWN_ID | *KEY* | *TOKEN* | *SECRET* | *PASS* | *CREDENTIAL* | *AUTH*)
		skipped="$skipped $name"
		continue
		;;
	esac
	if [ -n "$model" ]; then
		case $name in
		COPILOT_PROVIDER_*)
			if [ -z "$claude" ]; then
				dropped="$dropped $name"
				continue
			fi
			;;
		esac
		case $name in
		COPILOT_PROVIDER_MODEL_ID) value=$base ;;
		COPILOT_PROVIDER_MAX_PROMPT_TOKENS) [ -z "$limit" ] || value=$limit ;;
		esac
	fi
	printf 'export %s=%s\n' "$name" "$(quote "$value")" >>"$envfile"
	carried="$carried $name"
done

echo "relaunching $seat in $pane on$set_flags, resuming $session"
echo "carrying:$carried"
[ -z "$skipped" ] || echo "not carrying (spawn id or secret-like):$skipped"
[ -z "$dropped" ] || echo "dropped ($model is not Claude, so it runs plain Copilot):$dropped"
if [ -n "$claude" ] && [ -z "$proxied" ]; then
	echo "the seat ran plain, so it stays plain: only a pij spawn or revive runs the cache-proxy gate"
fi
command=". $(quote "$envfile"); rm -f $(quote "$envfile"); exec $(quote "$launcher")$words$set_flags $(quote "--resume=$session")"
# respawn-pane -k ends only this pane's processes (never kill by name: AGENTS.md, D-C34-3). The pane deletes the file.
tmux respawn-pane -k -t "$pane" -c "$folder" "$command"
trap - EXIT

# Wait, at most about a minute, for the seat to register on its new process.
tries=0
while [ "$tries" -lt 30 ]; do
	new=$(pij list --json | jq -r --arg s "$seat" '.data.seats[] | select(.id == $s) | .proc.pid')
	if [ -n "$new" ] && [ "$new" != null ] && [ "$new" != "$seat_pid" ]; then
		echo "registered on pid $new"
		exit 0
	fi
	/bin/sleep 2
	tries=$((tries + 1))
done
echo "$seat did not register a new process within a minute; look at its pane" >&2
exit 1
