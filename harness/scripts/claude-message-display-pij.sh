#!/bin/sh
# Claude MessageDisplay hook (plan 226): hide a reply's closing pij block on
# screen and show one line instead. Display only: the stored message, and what
# the model and the Stop hook see, are unchanged. The Stop hook runs the block.
# Managed by pij: the daemon installs it at ~/.pij-rs/claude-message-display-pij.sh.
#
# Cost: Claude launches this once per flush of every reply. It finds pij-rs
# with shell builtins and execs it, so each flush stays one process; pij-rs
# keeps the fence state across flushes, in their order, and touches no daemon.
# Without pij-rs the reply shows as written. Never fails the hook.
set -u
exec 2>/dev/null

pij_rs=""
saved_ifs=$IFS
IFS=:
set -f
for entry in $PATH; do
    case $entry in /*) [ -x "$entry/pij-rs" ] && pij_rs=$entry/pij-rs && break ;; esac
done
IFS=$saved_ifs
set +f
[ -z "$pij_rs" ] && [ -x "${HOME:-}/.npm-global/bin/pij-rs" ] && pij_rs=${HOME:-}/.npm-global/bin/pij-rs
if [ -z "$pij_rs" ]; then
    # Read the flush anyway, as pij-rs would, so Claude's write never meets a
    # closed pipe (F173). Builtins only: this path launches nothing either.
    while IFS= read -r line; do :; done
    exit 0
fi
exec "$pij_rs" hook claude-display
