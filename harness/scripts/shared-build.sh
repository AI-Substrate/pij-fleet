#!/usr/bin/env bash
# `just shared-build`: cargo into a target directory several worktrees share,
# one worktree at a time (plan 198).
#
# Cargo names a workspace crate's artifacts by crate, not by worktree, so two
# worktrees building into one target overwrite each other's pij_core and
# pij_store mid-build: missing symbols and false test failures. This holds an
# atomic mkdir lock for the whole batch, checks the disk floor, rebuilds the
# caller's own sources, and always releases.
#
#   PIJ_SHARED_TARGET=<absolute target dir> \
#     just shared-build [--timeout <minutes>|<seconds>s] -- <cargo args> [--then <cargo args>]...
#
# Environment:
#   PIJ_SHARED_TARGET   required, absolute; exported as CARGO_TARGET_DIR
#   PIJ_DISK_FLOOR_GB   refuse under this many GiB free on the target's volume (25)
#   PIJ_ID              seat id for the owner line (falls back to $USER)
#
# Exit: cargo's status, or 2 usage, 3 disk floor, 4 --timeout, 5 stale lock.
# A stale lock (its owner pid is gone) is reported with the command that clears
# it and never cleared here: whether that build is really over is the caller's
# call, not this script's.
#
# Cost: nothing at rest. Waiting: one df, one mkdir and one sleep per poll
# (20 s) per waiting caller. Holding: one df per cargo command.

set -uo pipefail

DF="${PIJ_SHARED_BUILD_DF:-/bin/df}"         # test seam
POLL="${PIJ_SHARED_BUILD_POLL_SECONDS:-20}"  # test seam
FLOOR_GB="${PIJ_DISK_FLOOR_GB:-25}"

refuse() {
	local code=$1
	shift
	printf 'shared-build: %s\n' "$*" >&2
	exit "$code"
}

usage() {
	printf '%s\n' "usage: PIJ_SHARED_TARGET=<absolute dir> just shared-build [--timeout <minutes>] -- <cargo args> [--then <cargo args>]..."
}

timeout_min=""
while [ $# -gt 0 ]; do
	case "$1" in
	--timeout)
		[ $# -ge 2 ] || refuse 2 "--timeout needs a number of minutes"
		timeout_min=$2
		shift 2
		;;
	--timeout=*)
		timeout_min=${1#--timeout=}
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	--)
		shift
		break
		;;
	-*) refuse 2 "unknown option $1 (cargo arguments go after --)" ;;
	*) break ;;
	esac
done
[ $# -gt 0 ] || refuse 2 "no cargo command; $(usage)"
timeout_secs=""
case "$timeout_min" in
"") ;;
*s) timeout_secs=${timeout_min%s} ;;
*) timeout_secs=$((10#${timeout_min//[!0-9]/0} * 60)) ;;
esac
case "$timeout_min" in
"" | [1-9] | [1-9]*[0-9] | [1-9]*s) ;;
*) refuse 2 "--timeout takes whole minutes (or seconds with an s suffix), not '$timeout_min'" ;;
esac
case "${timeout_min%s}" in
*[!0-9]*) refuse 2 "--timeout takes whole minutes (or seconds with an s suffix), not '$timeout_min'" ;;
esac
case "$FLOOR_GB" in
"" | *[!0-9]*) refuse 2 "PIJ_DISK_FLOOR_GB must be a whole number of GiB, not '$FLOOR_GB'" ;;
esac
case "$POLL" in
"" | *[!0-9]* | 0) refuse 2 "PIJ_SHARED_BUILD_POLL_SECONDS must be a positive whole number" ;;
esac

target="${PIJ_SHARED_TARGET:-}"
[ -n "$target" ] || refuse 2 "PIJ_SHARED_TARGET is not set: name the shared cargo target directory (absolute)"
case "$target" in
/*) ;;
*) refuse 2 "PIJ_SHARED_TARGET must be absolute, not '$target'" ;;
esac
target="${target%/}"
lock="$target.lock"

# One cargo command per element boundary: `--then` separates commands.
commands=()
current=()
for arg in "$@"; do
	if [ "$arg" = "--then" ]; then
		[ ${#current[@]} -gt 0 ] || refuse 2 "--then with no cargo command before it"
		commands+=("$(printf '%q ' "${current[@]}")")
		current=()
	else
		current+=("$arg")
	fi
done
[ ${#current[@]} -gt 0 ] || refuse 2 "--then with no cargo command after it"
commands+=("$(printf '%q ' "${current[@]}")")

worktree=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
seat="${PIJ_ID:-${USER:-unknown}}"

held=0
child=""

release() {
	if [ "$held" = 1 ]; then
		/bin/rm -rf "$lock"
		held=0
	fi
}

on_signal() {
	if [ -n "$child" ]; then
		kill -TERM "$child" 2>/dev/null
		wait "$child" 2>/dev/null
	fi
	release
	exit "$1"
}

trap release EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

# Free KiB on the volume holding the target (or its nearest existing parent).
# Builtins only past the one df: this runs once per poll while waiting.
free_kib() {
	local path=$target header avail
	while [ ! -e "$path" ] && [ "$path" != "/" ]; do
		path=${path%/*}
		[ -n "$path" ] || path=/
	done
	{
		read -r header
		read -r _ _ _ avail _
	} < <("$DF" -Pk "$path")
	printf '%s' "${avail:-}"
}

check_floor() {
	local free whole
	free=$(free_kib)
	case "$free" in
	"" | *[!0-9]*) refuse 3 "could not read free space for $target from $DF" ;;
	esac
	if [ "$free" -lt $((FLOOR_GB * 1048576)) ]; then
		release
		whole=$((free * 100 / 1048576))
		refuse 3 "disk floor: $((whole / 100)).$(printf '%02d' $((whole % 100))) GiB free under $target is below PIJ_DISK_FLOOR_GB=$FLOOR_GB; no cargo run"
	fi
}

announced=0
while :; do
	check_floor
	if /bin/mkdir "$lock" 2>/dev/null; then
		held=1
		break
	fi
	owner=""
	[ -r "$lock/owner" ] && read -r owner <"$lock/owner"
	if [[ "$owner" =~ pid=([0-9]+) ]] && ! kill -0 "${BASH_REMATCH[1]}" 2>/dev/null; then
		refuse 5 "stale lock $lock: its owner is gone ($owner). If no build is running into $target, clear it with: rm -rf '$lock'"
	fi
	if [ "$announced" = 0 ]; then
		printf 'shared-build: waiting for %s, held by: %s\n' "$lock" "${owner:-an owner still writing its line}" >&2
		announced=1
	fi
	if [ -n "$timeout_secs" ] && [ "$SECONDS" -ge "$timeout_secs" ]; then
		refuse 4 "gave up after --timeout $timeout_min waiting for $lock, held by: ${owner:-unknown}"
	fi
	/bin/sleep "$POLL"
done

printf 'seat=%s pid=%s worktree=%s started=%s\n' "$seat" "$$" "$worktree" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$lock/owner"

# The other tree's artifacts carry newer mtimes than this tree's sources; touch
# them so cargo rebuilds what this worktree actually holds.
sources=()
for file in "$worktree"/crates/*/src/lib.rs "$worktree"/crates/*/src/main.rs; do
	[ -f "$file" ] && sources+=("$file")
done
[ ${#sources[@]} -eq 0 ] || touch "${sources[@]}"

export CARGO_TARGET_DIR="$target"
export CARGO_INCREMENTAL=0
for command in "${commands[@]}"; do
	check_floor
	eval "set -- $command"
	# In the background under `wait`, so a signal releases the lock at once
	# rather than after cargo finishes.
	cargo "$@" &
	child=$!
	wait "$child"
	status=$?
	child=""
	[ "$status" -eq 0 ] || exit "$status"
done
