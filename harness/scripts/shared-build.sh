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
#   PIJ_SHARED_BUILD_TEST_LIMIT
#                       the longest one `cargo test` command may run from its
#                       start, compiling included, in whole minutes (or
#                       seconds with an s suffix), stretched by
#                       PIJ_TEST_TIME_SCALE; 0 for none. Default 20 (F273).
#                       Set, it is the only time limit: no filtered-run limit.
#   PIJ_SHARED_BUILD_FILTERED_TEST_LIMIT
#                       the longest a `cargo test` with a test name filter (any
#                       positional filter, before or after `--`) may run from
#                       its first test binary on; same form. Default 3 (F281).
#   PIJ_SHARED_BUILD_TEST_IDLE
#                       the longest any `cargo test` may write nothing (stdout
#                       or stderr) once its first test binary started, whatever
#                       the other limits; same form, 0 for off. Default 2 (F281).
#
# Exit: cargo's status, or 2 usage, 3 disk floor, 4 --timeout, 5 stale lock,
# 6 the --copy-out copy failed, 7 the build stamp could be neither written
# nor removed, 8 a `cargo test` ran past a limit.
#
# F273: one hung test held the lock for 41 minutes with four builds waiting.
# A `cargo test` past a limit is stopped: the script names the limit, the last
# `test ` line seen, and every process still running in cargo's own process
# group (the test binary, and any child it re-executed, by its command line),
# stops that group and only it (never anything matched by name), and releases
# the lock. F281: a filtered run that hung (a mutant run, 10+ min at 0% CPU,
# well inside the 20 minutes) held the lock and blocked another seat's build,
# so a filtered run gets 3 minutes once its tests start, and any run that
# falls silent for 2 minutes stops. Strict on purpose: raise a limit when real
# runs hit it. POLICIES on
# elapsed time and on silence, each deciding only over this run's own group.
# Cost: per `cargo test` command, one monitor and two relays (bash subshells,
# no exec, each blocked in `read`), the three FIFOs made once per run (one
# mkfifo), and one ps only when a limit falls. Silence is detected by the
# monitor's `read -t` returning at its deadline with no line: no polling. The
# relays copy cargo's output a line at a time (bash reads a pipe a byte per
# call, so CPU grows with output: 40,000 test lines, 1.3 MB, took 3.8 s of
# wall time and about 6 s of CPU on an M-series Mac). The F273 watchdog's
# /bin/sleep is gone. Ending a command waits for its relays in 0.05 s steps,
# at most 2 s.
#
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

# F273, F281: the limits of one `cargo test`, in seconds, scaled; empty for
# none. limit_secs <variable> <value> prints the seconds, or refuses.
limit_secs() {
	local name=$1 value=$2 secs scale
	case "$value" in
	0) return 0 ;;
	[1-9]*s) secs=${value%s} ;;
	[1-9]*) secs=$value ;;
	*) secs="bad" ;;
	esac
	case "$secs" in
	*[!0-9]*) refuse 2 "$name takes whole minutes (or seconds with an s suffix), or 0 for none, not '$value'" ;;
	esac
	case "$value" in
	*s) ;;
	*) secs=$((10#$secs * 60)) ;;
	esac
	scale="${PIJ_TEST_TIME_SCALE:-}"
	scale="${scale// /}"
	if [ -n "$scale" ] && [ "$scale" != 1 ]; then
		secs=$(/usr/bin/awk -v s="$scale" -v t="$secs" \
			'BEGIN { if (s !~ /^[0-9]+(\.[0-9]+)?$/ || s + 0 < 1) exit 1; printf "%d", t * s + 0.5 }') ||
			refuse 2 "PIJ_TEST_TIME_SCALE must be a number of at least 1, not '$scale'"
	fi
	printf '%s' "$secs"
}
# The whole command, from its start (compiling included): an explicit
# PIJ_SHARED_BUILD_TEST_LIMIT, else 20 minutes (F273).
TEST_LIMIT="${PIJ_SHARED_BUILD_TEST_LIMIT:-20}"
test_limit_secs=$(limit_secs PIJ_SHARED_BUILD_TEST_LIMIT "$TEST_LIMIT") || exit $?
# F281: a run with a test name filter, from its first test binary on, unless
# PIJ_SHARED_BUILD_TEST_LIMIT is set (an explicit limit is the only one).
FILTERED_LIMIT="${PIJ_SHARED_BUILD_FILTERED_TEST_LIMIT:-3}"
filtered_limit_secs=$(limit_secs PIJ_SHARED_BUILD_FILTERED_TEST_LIMIT "$FILTERED_LIMIT") || exit $?
[ -z "${PIJ_SHARED_BUILD_TEST_LIMIT:-}" ] || filtered_limit_secs=""
# F281: no output at all for this long, from its first test binary on,
# whatever the other limits.
IDLE_LIMIT="${PIJ_SHARED_BUILD_TEST_IDLE:-2}"
idle_limit_secs=$(limit_secs PIJ_SHARED_BUILD_TEST_IDLE "$IDLE_LIMIT") || exit $?

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
watchdog=""
relays=()
overran=0

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

# F273: the test limit's watchdog runs in its own process group (a subshell
# and its sleep); this stops that group, which is this script's own job, and
# returns once the group is gone, so a USR2 it sent is already pending here.
# Never `wait` on it: bash 5.2 (Linux) can block for good in `wait` on a
# watchdog that has signalled and exited (1 run in ~20 under load, #326 CI).
stop_watchdog() {
	[ -n "$watchdog" ] || return 0
	kill -TERM -- "-$watchdog" 2>/dev/null
	while kill -0 -- "-$watchdog" 2>/dev/null; do
		/bin/sleep 0.05
	done
	watchdog=""
}

on_signal() {
	stop_watchdog
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
	stop_watchdog
	[ -z "$child" ] || drain "$child"
	release
}

trap on_exit EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
# A closed terminal sends HUP: cargo runs in its own group and would otherwise
# keep building, holding the lock's target, with nothing waiting (F48).
trap 'on_signal 129' HUP
# The watchdog's signal that a test command ran past its limit (F273).
trap 'overran=1' USR2

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
# cargo finishes; `set -m` gives the job its own process group. A `cargo test`
# with any limit gets a monitor (F273, F281), in its own group too: when a
# limit falls it signals this script, whose trap ends the `wait`.
#
# F281: the monitor sees cargo's output. Cargo writes its stdout and stderr to
# two FIFOs in the lock (made once per run, one /usr/bin/mkfifo); a relay on
# each copies every line to this script's own stdout or stderr, unchanged and
# in order, and passes it to the monitor through a third FIFO. The monitor
# blocks in `read -t` until the next line or the nearest deadline, whichever
# comes first, so idleness costs no polling: no line before the deadline IS
# the idle limit. Its clocks: the test limit from the command's start; the
# filtered-run limit and the idle limit from cargo's first `Running` line (the
# first test binary), so compiling counts only against the test limit.
# Output through a pipe, not a terminal: cargo and libtest print no colour.
USR2_NUMBER=$(kill -l USR2)
run_cargo() { # <cargo args>...
	local status filtered=""
	# A USR2 from an earlier command's monitor has run its trap by now
	# (stop_watchdog returned only once that monitor was gone): never this
	# command's overrun.
	overran=0
	[ "$1" = test ] && has_filter "$@" && filtered=$filtered_limit_secs
	if [ "$1" != test ] || [ -z "$test_limit_secs$filtered$idle_limit_secs" ]; then
		set -m
		${niced[@]+"${niced[@]}"} cargo "$@" &
		child=$!
		set +m
		wait_cargo "$@"
		status=$?
		drain "$child"
		child=""
		return "$status"
	fi
	if [ ! -p "$lock/act" ]; then
		/usr/bin/mkfifo "$lock/out" "$lock/err" "$lock/act" || refuse 2 "could not make the output FIFOs in $lock"
	fi
	: >"$lock/stopped"
	set -m
	monitor "$test_limit_secs" "$filtered" "$idle_limit_secs" <"$lock/act" &
	watchdog=$!
	relay 1 <"$lock/out" &
	relays=("$!")
	relay 2 <"$lock/err" &
	relays+=("$!")
	${niced[@]+"${niced[@]}"} cargo "$@" >"$lock/out" 2>"$lock/err" &
	child=$!
	set +m
	wait_cargo "$@"
	status=$?
	# Whatever cargo left running in its group must not outlive the hold.
	drain "$child"
	child=""
	finish_relays
	stop_watchdog
	return "$status"
}

# Wait for cargo, through any USR2 a limit sends: past a limit with cargo's
# group still running, overrun; a group that ended as the limit fell waits
# again for cargo's own status.
wait_cargo() { # <cargo args>...
	local status
	while :; do
		wait "$child"
		status=$?
		[ "$overran" = 1 ] || break
		overran=0
		kill -0 -- "-$child" 2>/dev/null && overrun "$@"
		[ "$status" -eq $((128 + USR2_NUMBER)) ] || break
	done
	return "$status"
}

# F281: whether `cargo test <args>` filters tests by name: a positional
# argument, before `--` (cargo's TESTNAME) or after it (libtest's filters),
# that is not an option's value.
has_filter() { # test <args>...
	local arg after="" value=""
	shift
	for arg in "$@"; do
		if [ -n "$value" ]; then
			value=""
			continue
		fi
		case "$after:$arg" in
		:--) after=1 ;;
		*:-*=*) ;;
		:-p | :--package | :--exclude | :--test | :--bench | :--bin | :--example | :-F | :--features | \
			:--target | :--target-dir | :--manifest-path | :-j | :--jobs | :--profile | :--color | \
			:--message-format | :--config | :-Z | :--lockfile-path | \
			1:--test-threads | 1:--skip | 1:--format | 1:--logfile | 1:--color | 1:-Z | 1:--shuffle-seed)
			value=1
			;;
		*:-*) ;;
		*) return 0 ;;
		esac
	done
	return 1
}

# F281: copy each line from stdin to fd <fd> unchanged, and pass it to the
# monitor: a `Running` or `test ` line whole, any other as an empty line.
# Writes to a monitor that has gone are dropped, never the copy.
relay() { # <fd>
	local line
	trap '' PIPE
	exec 7>"$lock/act"
	while IFS= read -r line || [ -n "$line" ]; do
		printf '%s\n' "$line" >&"$1"
		case "$line" in
		*"Running "* | "test "*) printf '%s\n' "$line" >&7 ;;
		*) printf '\n' >&7 ;;
		esac 2>/dev/null
	done
}

# Each relay ends once cargo's group has closed its FIFO. A descendant that
# escaped the group could hold it open, so the wait is bounded (2 s), then
# the relay is stopped.
finish_relays() {
	local pid tick=0
	for pid in ${relays[@]+"${relays[@]}"}; do
		while kill -0 "$pid" 2>/dev/null; do
			tick=$((tick + 1))
			[ "$tick" -lt 40 ] || kill -TERM "$pid" 2>/dev/null
			/bin/sleep 0.05
		done
	done
	relays=()
}

# F273, F281: the limits of one `cargo test`, on the lines its relays pass:
# <test limit> from the start; <filtered limit> and <idle limit> once a
# `Running` line has been seen. Blocks in `read -t` until a line or the
# nearest deadline. When one falls it writes which, and the last `test ` line,
# to the lock, signals this script and ends. Ends too when the relays do.
# bash 3.2 (macOS /bin/bash) answers 1 for both a timeout and the end of
# input, so a failed read that took no time is taken as the end only when the
# read after it fails at once too; a timeout lets the loop find the deadline.
monitor() { # <test secs> <filtered secs> <idle secs>
	local overall=$1 filtered=$2 idle=$3 start=$SECONDS armed="" active=$SECONDS
	local line="" last="" due which status before ended=""
	while :; do
		due=""
		which=""
		if [ -n "$overall" ]; then
			due=$((start + overall))
			which=test
		fi
		if [ -n "$armed" ] && [ -n "$filtered" ] && { [ -z "$due" ] || [ $((armed + filtered)) -lt "$due" ]; }; then
			due=$((armed + filtered))
			which=filtered
		fi
		if [ -n "$armed" ] && [ -n "$idle" ] && { [ -z "$due" ] || [ $((active + idle)) -lt "$due" ]; }; then
			due=$((active + idle))
			which=idle
		fi
		if [ -z "$which" ]; then
			IFS= read -r line || return 0
		elif [ "$due" -le "$SECONDS" ]; then
			printf '%s\n%s\n' "$which" "$last" >"$lock/stopped"
			kill -USR2 "$$"
			return 0
		else
			before=$SECONDS
			IFS= read -r -t $((due - SECONDS)) line
			status=$?
			if [ "$status" -ne 0 ]; then
				if [ "$status" -le 128 ] && [ "$SECONDS" -eq "$before" ]; then
					[ -z "$ended" ] || return 0
					ended=1
				fi
				continue
			fi
			ended=""
		fi
		active=$SECONDS
		case "$line" in
		*"Running "*) [ -n "$armed" ] || armed=$SECONDS ;;
		"test "*) last=$line ;;
		esac
	done
}

# F273, F281: name the limit that fell and the last test line seen, and what
# still runs in cargo's group (the test binary, and any child it re-executed,
# by command line); stop that group and only it, and leave the lock to the
# EXIT trap. Exit 8.
overrun() { # <cargo args>...
	local running="" pgid rest which="test" last=""
	while read -r pgid rest; do
		[ "$pgid" = "$child" ] && running="$running
  $rest"
	done < <(/bin/ps -ax -o pgid=,pid=,etime=,command=)
	[ -s "$lock/stopped" ] && { read -r which && read -r last; } <"$lock/stopped"
	case "$which" in
	idle)
		printf 'shared-build: cargo %s wrote nothing for %ss, its idle limit (PIJ_SHARED_BUILD_TEST_IDLE=%s, PIJ_TEST_TIME_SCALE=%s)' \
			"$*" "$idle_limit_secs" "$IDLE_LIMIT" "${PIJ_TEST_TIME_SCALE:-1}" >&2
		;;
	filtered)
		printf 'shared-build: cargo %s ran past its filtered-run limit of %ss after its first test binary started (a test name filter; PIJ_SHARED_BUILD_FILTERED_TEST_LIMIT=%s, PIJ_TEST_TIME_SCALE=%s)' \
			"$*" "$filtered_limit_secs" "$FILTERED_LIMIT" "${PIJ_TEST_TIME_SCALE:-1}" >&2
		;;
	*)
		printf 'shared-build: cargo %s ran past its test limit of %ss (PIJ_SHARED_BUILD_TEST_LIMIT=%s, PIJ_TEST_TIME_SCALE=%s)' \
			"$*" "$test_limit_secs" "$TEST_LIMIT" "${PIJ_TEST_TIME_SCALE:-1}" >&2
		;;
	esac
	printf '; last test line: %s; still running in its process group %s (pid, elapsed, command):%s\n' \
		"${last:-(none)}" "$child" "$running" >&2
	printf 'shared-build: stopping that process group, and only it, then releasing %s\n' "$lock" >&2
	drain "$child"
	wait "$child" 2>/dev/null
	child=""
	finish_relays
	stop_watchdog
	exit 8
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
