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

current=$(/bin/ps -o nice= -p $$ 2>/dev/null)
# Trim ps's padding without another launch.
current=${current#"${current%%[![:space:]]*}"}
current=${current%"${current##*[![:space:]]}"}
case "$current" in
"" | *[!0-9-]* | -) current=0 ;;
esac
if [ "$current" -ge 19 ]; then
	exec "$@"
fi
exec /usr/bin/nice -n $((19 - current)) "$@"
