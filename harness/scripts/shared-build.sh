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
# Builds run niced. A test that times real child processes (ps, tmux, spawn
# timing) may instead run from an already-built binary under `nice -n 19`
# alone, its budgets stretched by PIJ_TEST_TIME_SCALE (plan 209, F28; AGENTS.md,
# resource cost item 5).
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
MKDIR="${PIJ_SHARED_BUILD_MKDIR:-/bin/mkdir}" # test seam
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
# Two spellings of one directory (a symlink and its destination) must take one
# lock, so the lock and CARGO_TARGET_DIR derive from the physical path. A
# target that does not exist yet resolves through its nearest existing parent.
# One spelling of a path: no doubled and no trailing separators, so `target/`
# and `target` name one lock whether or not the directory exists yet.
normal_path() {
	local path=$1
	while [ "${path#*//}" != "$path" ]; do
		path=${path%%//*}/${path#*//}
	done
	while [ "$path" != "/" ] && [ "${path%/}" != "$path" ]; do
		path=${path%/}
	done
	printf '%s' "$path"
}

physical_target() {
	local path rest="" resolved
	path=$(normal_path "$1")
	if [ -L "$path" ] && [ ! -e "$path" ]; then
		refuse 2 "PIJ_SHARED_TARGET '$1' is a dangling symlink"
	fi
	while [ ! -d "$path" ]; do
		[ ! -e "$path" ] || refuse 2 "PIJ_SHARED_TARGET '$1' is not a directory"
		rest="/${path##*/}$rest"
		path=${path%/*}
		[ -n "$path" ] || path=/
	done
	case "$rest" in
	*/. | */./* | */.. | */../*) refuse 2 "PIJ_SHARED_TARGET '$1' has a . or .. below its existing part" ;;
	esac
	resolved=$(cd -P "$path" && pwd -P) || refuse 2 "could not resolve PIJ_SHARED_TARGET '$1'"
	normal_path "$resolved$rest"
}

target=$(physical_target "$target") || exit $?
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

# Cargo runs in its own process group (job control below), so stopping it
# reaches rustc, build scripts and linkers, not just cargo's own pid. The lock
# is released only once that whole group is gone: a descendant that outlived
# its cargo could otherwise write into the next tree's build.
DRAIN_TICKS=100 # 0.1 s each: TERM, then KILL after 10 s
drain() {
	local group=$1 tick=0
	kill -0 -- "-$group" 2>/dev/null || return 0
	kill -TERM -- "-$group" 2>/dev/null
	while kill -0 -- "-$group" 2>/dev/null; do
		tick=$((tick + 1))
		if [ "$tick" -ge "$DRAIN_TICKS" ]; then
			kill -KILL -- "-$group" 2>/dev/null
		fi
		/bin/sleep 0.1
	done
}

on_signal() {
	if [ -n "$child" ]; then
		drain "$child"
		wait "$child" 2>/dev/null
	fi
	release
	exit "$1"
}

trap release EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
# A closed terminal sends HUP: cargo runs in its own group and would otherwise
# keep building, holding the lock's target, with nothing waiting (F48).
trap 'on_signal 129' HUP

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

# A target that does not exist yet may not have a parent either.
/bin/mkdir -p "${lock%/*}" || refuse 2 "could not create the directory for $lock"

# The lock is created 0700 in the mkdir itself, never chmod-ed afterwards: under
# a permissive umask (002) a group-writable lock would let another user plant
# a symlink at the owner file before it is written. Other users still wait on
# it; they just cannot read its owner line.
announced=0
while :; do
	check_floor
	if "$MKDIR" -m 0700 "$lock" 2>/dev/null; then
		held=1
		break
	fi
	# A failed mkdir that left no lock is not a holder. The holder may have
	# released between the two calls, so try once more before refusing.
	if [ ! -d "$lock" ]; then
		if "$MKDIR" -m 0700 "$lock" 2>/dev/null; then
			held=1
			break
		fi
		[ -d "$lock" ] || refuse 2 "could not create the lock $lock"
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

# Never follow or overwrite whatever sits at the owner path: anything there
# already was not written by this hold. noclobber makes the create exclusive.
owner_line="seat=$seat pid=$$ worktree=$worktree started=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
if [ -e "$lock/owner" ] || [ -L "$lock/owner" ] ||
	! (set -o noclobber && printf '%s\n' "$owner_line" >"$lock/owner") 2>/dev/null; then
	release
	refuse 2 "could not write the owner line in $lock: something already sits at $lock/owner; no cargo run"
fi

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
	# In the background under `wait`, so a signal is handled at once rather
	# than after cargo finishes; `set -m` gives the job its own process group.
	set -m
	cargo "$@" &
	child=$!
	set +m
	wait "$child"
	status=$?
	# Whatever cargo left running in its group must not outlive the hold.
	drain "$child"
	child=""
	[ "$status" -eq 0 ] || exit "$status"
done
