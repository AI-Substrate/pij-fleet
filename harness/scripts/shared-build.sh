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
# Every cargo command runs under `nice -n 19` only (below); a test's
# wall-clock budgets on real child processes stretch with PIJ_TEST_TIME_SCALE
# (plan 209, F28; AGENTS.md, resource cost item 5).
#
#   PIJ_SHARED_TARGET=<absolute target dir> \
#     just shared-build [--timeout <minutes>|<seconds>s] [--copy-out <artifact> <dest>] -- <cargo args> [--then <cargo args>]...
#
# --copy-out copies <artifact> (relative to the target) to <dest> after every
# cargo command succeeded, still holding the lock, so the copy is the bytes this
# run built: no other tree's build into the same target can land in between
# (F226, R271-001). <dest> must not exist yet.
#
# Environment:
#   PIJ_SHARED_TARGET   required, absolute; exported as CARGO_TARGET_DIR
#   PIJ_DISK_FLOOR_GB   refuse under this many GiB free on the target's volume (25)
#   PIJ_CARGO_JOBS      cargo's job cap, exported as CARGO_BUILD_JOBS (4), and
#                       the test-thread cap, exported as RUST_TEST_THREADS
#                       unless that is already set (F126)
#   PIJ_ID              seat id for the owner line; else $PIJ_SESSION_ID (set in
#                       every harness seat), else `pij-rs whoami`, else $USER
#
# Exit: cargo's status, or 2 usage, 3 disk floor, 4 --timeout, 5 stale lock,
# 6 the --copy-out copy failed, 7 the build stamp could be neither written
# nor removed.
# A stale lock (its owner pid is gone) is reported with the command that clears
# it and never cleared here: whether that build is really over is the caller's
# call, not this script's.
#
# Parallelism is capped (F126): load reached about 95 and starved every
# project's gates, and `nice` does not shorten the run queue. The repository's
# `.cargo/config.toml` sets the default (4 jobs, 4 test threads) for every cargo
# run; this maps PIJ_CARGO_JOBS onto both, as CARGO_BUILD_JOBS (which beats the
# config) and RUST_TEST_THREADS (libtest's env form of --test-threads). An
# explicit `-- --test-threads N` still wins, as does a RUST_TEST_THREADS the
# caller set. The cap is printed first.
#
# Every cargo command runs at niceness max(19, the caller's), through
# harness/scripts/nice19.sh (F220), which rustc, linkers and test binaries
# inherit; it never stacks to 20. Each runs as given: one step each.
# No `taskpolicy -b` (F186, Jordan's ruling of 2026-11-11): background QoS
# confined cargo to the efficiency cores and throttled its IO, so at fleet load
# builds starved for 20 to 35 minutes. The lock (one build at a time) and the
# jobs cap bound what a build takes instead. This supersedes plan 208's
# background-QoS rule and plan 209's `--no-run` compile step (F63), which only
# existed to keep tests out of background QoS.
#
# Cost: nothing at rest. A build uses the performance cores at nice 19, one at
# a time, with at most PIJ_CARGO_JOBS rustc jobs and test threads per binary
# (F126). Waiting: one df, one mkdir and one sleep per poll (20 s) per waiting
# caller. Holding: one df and one nice19.sh per cargo command (one ps, then
# nice execs into cargo), never per poll.

set -uo pipefail

DF="${PIJ_SHARED_BUILD_DF:-/bin/df}"         # test seam
MKDIR="${PIJ_SHARED_BUILD_MKDIR:-/bin/mkdir}" # test seam
POLL="${PIJ_SHARED_BUILD_POLL_SECONDS:-20}"  # test seam
WHOAMI="${PIJ_SHARED_BUILD_WHOAMI-}"          # test seam; default: pij-rs on PATH
MKTEMP="${PIJ_SHARED_BUILD_MKTEMP:-/usr/bin/mktemp}" # test seam
FLOOR_GB="${PIJ_DISK_FLOOR_GB:-25}"
JOBS="${PIJ_CARGO_JOBS:-4}"

refuse() {
	local code=$1
	shift
	printf 'shared-build: %s\n' "$*" >&2
	exit "$code"
}

usage() {
	printf '%s\n' "usage: PIJ_SHARED_TARGET=<absolute dir> just shared-build [--timeout <minutes>] [--copy-out <artifact> <dest>] -- <cargo args> [--then <cargo args>]..."
}

timeout_min=""
copy_artifact=""
copy_dest=""
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
	--copy-out)
		[ $# -ge 3 ] || refuse 2 "--copy-out needs an artifact (relative to the target) and a destination"
		case "$2" in
		"" | /* | ../* | */../* | */..) refuse 2 "--copy-out takes an artifact path inside the target, not '$2'" ;;
		esac
		[ -n "$3" ] || refuse 2 "--copy-out needs a destination"
		copy_artifact=$2
		copy_dest=$3
		shift 3
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
case "$JOBS" in
"" | *[!0-9]*) refuse 2 "PIJ_CARGO_JOBS must be a positive whole number of jobs, not '$JOBS'" ;;
esac
JOBS=$((10#$JOBS))
[ "$JOBS" -gt 0 ] || refuse 2 "PIJ_CARGO_JOBS must be a positive whole number of jobs, not '${PIJ_CARGO_JOBS:-}'"
export CARGO_BUILD_JOBS="$JOBS"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-$JOBS}"
printf 'shared-build: cargo jobs: %s (PIJ_CARGO_JOBS), test threads: %s (RUST_TEST_THREADS)\n' "$CARGO_BUILD_JOBS" "$RUST_TEST_THREADS" >&2

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
# The owner line names the seat, so waiting seats can tell holders apart
# (plan 208, F24). whoami is asked at most once per run, never per poll.
seat_id() {
	local id="${PIJ_ID:-${PIJ_SESSION_ID:-}}"
	if [ -z "$id" ]; then
		local whoami="${WHOAMI:-$(command -v pij-rs 2>/dev/null)}"
		if [ -n "$whoami" ] && [ -x "$whoami" ]; then
			id=$("$whoami" whoami --json 2>/dev/null | grep -o '"id":"[^"]*"' | head -n 1 | cut -d '"' -f 4)
		fi
	fi
	case "$id" in
	"" | *[!A-Za-z0-9._-]*) id="${USER:-unknown}" ;;
	esac
	printf '%s' "$id"
}
seat=$(seat_id)

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
		child=""
	fi
	release
	exit "$1"
}

# Any other exit with cargo running (an untrapped signal such as USR1, which
# bash still runs this trap for) drains its group before the lock goes (F48).
on_exit() {
	[ -z "$child" ] || drain "$child"
	release
}

trap on_exit EXIT
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

# Cargo takes a crate as fresh when its artifact is newer than its sources.
# Another tree's artifacts are newer than this tree's sources, so after one
# this tree's crate roots are touched and cargo rebuilds what this worktree
# holds. Only then (F243): touching on every run rebuilt every workspace crate
# every time, about 80 s of each release build.
#
# `stamp` holds `running <tree>` from before a run's first cargo command and
# `finished <tree>` once its last one finished, so only "this tree finished
# the last run here" skips the touch. A run cut short, or a batch whose later
# command failed after an earlier one built (R297-001), leaves `running …`
# behind and the next run touches, whoever it is. A stamp in any other form
# (round 1's bare `<tree>`, which a failed batch could leave trusted and
# newer than another tree's artifacts) never matches, so each tree touches
# once on adoption. Anything that built here outside a run
# (bare cargo) left an artifact newer than the stamp in some `deps`, in any
# cargo layout (<profile>/deps, <triple>/<profile>/deps): one find over the
# target, at any depth, skipping cargo's own bookkeeping (build scripts,
# incremental caches, fingerprints) and stopping at the first newer entry.
stamp="$target/.pij-shared-build-tree"
# The stamp is written to an exclusive temp file (mktemp, in the target) and
# renamed over the stamp's path. A link or a directory planted at that path
# is never followed or written into: no stamp is written, so every run
# touches (S297, sure-mole). The check and the rename are two steps; anyone
# who can plant one there between them can already write the target itself.
# A stamp that can't be written (mktemp, the write or the rename failed) is
# removed instead, with a warning, so no earlier `finished …` survives a run
# it does not describe and the next run touches (F246). If it can't be
# removed either, it may still say `finished <tree>`: the `running` write
# refuses the run before any cargo, and after the last cargo it already says
# `running …`, which never skips the touch.
write_stamp() { # <content>; fails only when a stale stamp may remain
	[ -d "$target" ] || return 0
	if [ -L "$stamp" ] || [ -d "$stamp" ]; then return 0; fi
	local tmp=""
	if tmp=$("$MKTEMP" "$target/.pij-shared-build-tree.XXXXXX") &&
		printf '%s\n' "$1" >"$tmp" && /bin/mv -f "$tmp" "$stamp"; then
		return 0
	fi
	[ -z "$tmp" ] || /bin/rm -f "$tmp"
	/bin/rm -f "$stamp" || return 1
	printf 'shared-build: could not write %s; removed it, so the next run touches the crate roots\n' "$stamp" >&2
}
last=""
[ -f "$stamp" ] && [ ! -L "$stamp" ] && read -r last <"$stamp"
if [ "$last" != "finished $worktree" ] ||
	[ -n "$(/usr/bin/find "$target" \( -name build -o -name incremental -o -name .fingerprint \) -prune -o -path '*/deps/*' -newer "$stamp" -print -quit 2>/dev/null)" ]; then
	sources=()
	for file in "$worktree"/crates/*/src/lib.rs "$worktree"/crates/*/src/main.rs; do
		[ -f "$file" ] && sources+=("$file")
	done
	[ ${#sources[@]} -eq 0 ] || touch "${sources[@]}"
fi
write_stamp "running $worktree" ||
	refuse 7 "could not write or remove $stamp, which may skip the next run's touch; no cargo run"

# Every cargo command runs at niceness max(19, the caller's) (F186, F220): the
# wrapper adds 19 minus the caller's niceness, so `just test` (already at 19)
# adds nothing instead of stacking to 20 (R257-001).
here=${BASH_SOURCE[0]%/*}
[ "$here" != "${BASH_SOURCE[0]}" ] || here=.
niced=(/bin/sh "$here/nice19.sh")

# One cargo command, niced, in its own process group, drained after. In the
# background under `wait`, so a signal is handled at once rather than after
# cargo finishes; `set -m` gives the job its own process group.
run_cargo() { # <cargo args>...
	local status
	set -m
	${niced[@]+"${niced[@]}"} cargo "$@" &
	child=$!
	set +m
	wait "$child"
	status=$?
	# Whatever cargo left running in its group must not outlive the hold.
	drain "$child"
	child=""
	return "$status"
}

export CARGO_TARGET_DIR="$target"
export CARGO_INCREMENTAL=0
for command in "${commands[@]}"; do
	check_floor
	eval "set -- $command"
	run_cargo "$@" || exit $?
done
# F243: this tree's run finished here. Written after the last cargo, so every
# artifact the run built is older than it; a failed run never gets here and
# leaves `running …`, so the next run, this tree's or another's, touches.
write_stamp "finished $worktree" ||
	printf 'shared-build: could not write %s; it still reads running, so the next run touches\n' "$stamp" >&2
# F226: the artifact this run built, copied while the lock is still held.
if [ -n "$copy_artifact" ]; then
	[ ! -e "$copy_dest" ] && [ ! -L "$copy_dest" ] ||
		refuse 6 "--copy-out: $copy_dest already exists; nothing copied"
	/bin/cp "$target/$copy_artifact" "$copy_dest" ||
		refuse 6 "--copy-out: could not copy $target/$copy_artifact to $copy_dest"
fi
