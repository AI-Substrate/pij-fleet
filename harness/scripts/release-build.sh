#!/bin/bash
# Build pij-rs for release from this checkout and print the path of a private,
# verified copy of it, or exit non-zero having printed nothing (F226).
#
#   release-build.sh
#
# The one deploy build: `just rs-install`, `just bounce-rs` and
# `pij-rs daemon bounce` all build through it, before anything is installed
# or any daemon is stopped. The caller installs the printed copy, then
# removes its directory.
#
# - Build inputs must be HEAD's: a tracked or untracked change under crates/,
#   Cargo.toml, Cargo.lock, .cargo/ or the stamped extension trees is refused,
#   since the stamp says HEAD (R271 review). Other local changes (config such as
#   .omp/models.yml) are not built in and do not block a deploy.
# - Through shared-build into this checkout's one target (cargo-target.sh,
#   F217): the target's lock, this tree's own sources rebuilt (another
#   worktree's artifacts there carry newer mtimes), niceness 19. At the F226
#   bounce a plain cargo build linked a pij-store built from another tree.
# - Stamped with this checkout's commit and extension trees (F113), on this
#   command only, never exported (#175 review, P175-F001).
# - A failed build exits with cargo's status. Nothing is piped, so nothing
#   hides it: `cargo build | grep | tail` exited 0 on a failed build and the
#   recipe installed a stale binary from another worktree.
# - The binary is copied out of the shared target while shared-build still
#   holds its lock, into a private directory, and that copy must answer
#   `--version` with exactly this commit's stamp. The bytes checked are the
#   bytes installed: another tree's later build into the shared target can't
#   replace them (R271-001). Anything else is refused and never handed on.
#
# Exit: cargo's status, shared-build's (2 usage, 3 disk floor, 4 timeout,
# 5 stale lock, 6 copy), or 1 for dirty build inputs or a wrongly stamped
# binary.
set -euo pipefail
here=$(cd "${BASH_SOURCE[0]%/*}" && pwd -P)
cd "$(git rev-parse --show-toplevel)"
snapshot=""
refuse() {
	[ -z "$snapshot" ] || rm -rf "$snapshot"
	printf 'release-build: %s; nothing was installed or stopped\n' "$1" >&2
	exit 1
}

dirty=$(git status --porcelain --untracked-files=all -- Cargo.toml Cargo.lock .cargo crates \
	.copilot/extensions/pij .omp/extensions/pij)
[ -z "$dirty" ] || refuse "build inputs differ from HEAD (commit or remove them first):
$dirty"

# F113: the commit, and the extension source trees the PA digest compares
# seats' extensions with (ten characters of each id; the OMP extension also
# loads crates/core/tests/fixtures/control-commands.json). One git launch.
ids=$(git rev-parse HEAD HEAD:.copilot/extensions/pij HEAD:.omp/extensions/pij HEAD:crates/core/tests/fixtures/control-commands.json)
set -- $ids
commit=${1:0:10}
copilot=${2:0:10}
omp="${3:0:10}+${4:0:10}"

target=$(bash "$here/cargo-target.sh")
snapshot=$(mktemp -d "${TMPDIR:-/tmp}/pij-release.XXXXXX")
copy="$snapshot/pij-rs"
status=0
PIJ_BUILD_COMMIT="$commit" PIJ_EXT_TREE_COPILOT="$copilot" PIJ_EXT_TREE_OMP="$omp" \
	PIJ_SHARED_TARGET="$target" \
	bash "$here/shared-build.sh" --copy-out release/pij-rs "$copy" \
	-- build --release --locked -p pij-cli --bin pij-rs >&2 || status=$?
if [ "$status" -ne 0 ]; then
	rm -rf "$snapshot"
	exit "$status"
fi

[ -x "$copy" ] || refuse "the build left no pij-rs to copy"
version=$("$copy" --version 2>/dev/null) || refuse "the built pij-rs does not answer --version"
case "$version" in
"pij-rs "*" ($commit)") ;;
*) refuse "the built pij-rs is '$version', not this checkout's commit $commit" ;;
esac
printf '%s\n' "$copy"
