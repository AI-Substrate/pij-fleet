#!/usr/bin/env bash
# The one cargo target this checkout builds into (F217). Prints an absolute path.
#
#   CARGO_TARGET_DIR    the target the outer cargo is already using, if set
#   PIJ_SHARED_TARGET   the shared target a seat named, if set
#   otherwise           the main checkout's target/, which every linked worktree
#                       of it shares
#
# Never a fresh per-worktree target: a debug build of the workspace is about
# 15 GB, so one per lane filled the disk under shared-build's floor. Callers
# build into it only through `just shared-build`, whose lock and source touch
# keep worktrees sharing it from building each other's crates.

set -euo pipefail

root=$(git rev-parse --show-toplevel)
target="${CARGO_TARGET_DIR:-${PIJ_SHARED_TARGET:-}}"
if [ -z "$target" ]; then
	common=$(git rev-parse --path-format=absolute --git-common-dir)
	target="${common%/*}/target"
fi
case "$target" in
/*) ;;
*) target="$root/$target" ;;
esac
printf '%s\n' "$target"
