#!/bin/sh
# Run a command at niceness 19 without stacking (F220): adds 19 minus the
# caller's niceness, so the command runs at max(19, caller). A plain
# `nice -n 19` inside a recipe the caller already niced ran at 20 on macOS
# (whose range ends at 20; Linux's at 19), which failed shared-build.test.ts
# (R257-001). Every just recipe and `just shared-build` nice through this.
#
#   harness/scripts/nice19.sh <command> [args...]
#
# Cost: one ps per call, then nice execs into the command (none at 19+).
#
# R257-002: a niceness this cannot read is refused, never guessed: reading it
# as 0 would run an already-niced caller's command at the old stacked level.
# NICE19_PS names the ps to ask (tests only; default /bin/ps).

ps=${NICE19_PS:-/bin/ps}
if ! current=$("$ps" -o nice= -p $$); then
	echo "nice19.sh: could not read this shell's niceness ($ps exited non-zero); nothing was run" >&2
	exit 1
fi
# Trim ps's padding without another launch.
current=${current#"${current%%[![:space:]]*}"}
current=${current%"${current##*[![:space:]]}"}
case "$current" in
-[0-9]* | [0-9]*)
	case "${current#-}" in
	*[!0-9]*) current="" ;;
	esac
	;;
*) current="" ;;
esac
if [ -z "$current" ]; then
	echo "nice19.sh: $ps gave no niceness this can read; nothing was run" >&2
	exit 1
fi
if [ "$current" -ge 19 ]; then
	exec "$@"
fi
exec /usr/bin/nice -n $((19 - current)) "$@"
