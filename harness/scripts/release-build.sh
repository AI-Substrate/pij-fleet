#!/bin/bash
# Build pij-rs for release from this checkout and print the built binary's
# path, or exit non-zero having printed nothing (F226).
#
#   release-build.sh
#
# The one deploy build: `just rs-install`, `just bounce-rs` and
# `pij-rs daemon bounce` all build through it, before anything is installed
# or any daemon is stopped.
#
# - Through shared-build into this checkout's one target (cargo-target.sh,
#   F217): the target's lock, this tree's own sources rebuilt (another
#   worktree's artifacts there carry newer mtimes), niceness 19. At the F226
#   bounce a plain cargo build linked a pij-store built from another tree.
# - Stamped with this checkout's commit and extension trees (F113), on this
#   command only, never exported (#175 review, P175-F001).
# - A failed build exits with cargo's status. Nothing is piped, so nothing
#   hides it: `cargo build | grep | tail` exited 0 on a failed build and the
#   recipe installed a stale binary from another worktree.
# - The built binary must answer `--version` with exactly this commit's stamp.
#   Anything else (unstamped, another commit) is refused and never handed on.
#
# Exit: cargo's status, shared-build's (2 usage, 3 disk floor, 4 timeout,
# 5 stale lock), or 1 for a missing or wrongly stamped binary.
set -euo pipefail
here=$(cd "${BASH_SOURCE[0]%/*}" && pwd -P)
refuse() {
	printf 'release-build: %s; nothing was installed or stopped\n' "$1" >&2
	exit 1
}

# F113: the commit, and the extension source trees the PA digest compares
# seats' extensions with (ten characters of each id; the OMP extension also
# loads crates/core/tests/fixtures/control-commands.json). One git launch.
ids=$(git rev-parse HEAD HEAD:.copilot/extensions/pij HEAD:.omp/extensions/pij HEAD:crates/core/tests/fixtures/control-commands.json)
set -- $ids
commit=${1:0:10}
copilot=${2:0:10}
omp="${3:0:10}+${4:0:10}"

target=$(bash "$here/cargo-target.sh")
PIJ_BUILD_COMMIT="$commit" PIJ_EXT_TREE_COPILOT="$copilot" PIJ_EXT_TREE_OMP="$omp" \
	PIJ_SHARED_TARGET="$target" \
	bash "$here/shared-build.sh" -- build --release --locked -p pij-cli --bin pij-rs >&2

built="$target/release/pij-rs"
[ -x "$built" ] || refuse "the build left no pij-rs at $built"
version=$("$built" --version 2>/dev/null) || refuse "$built does not answer --version"
case "$version" in
"pij-rs "*" ($commit)") ;;
*) refuse "$built is '$version', not this checkout's commit $commit" ;;
esac
printf '%s\n' "$built"
