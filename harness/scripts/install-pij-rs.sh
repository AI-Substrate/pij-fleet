#!/bin/sh
# Install a built pij-rs as a copy outside any cargo target/ (F187).
#
#   install-pij-rs.sh <built pij-rs> [<PATH dir>]
#
# The copy is $HOME/.local/share/pij/bin/pij-rs. Deleting or cleaning a
# target/ then never takes the CLI, or the daemon's next launch, down with it.
# The binary is copied under a temporary name and renamed into place, so a
# daemon running the old copy keeps its own file, and macOS never sees a signed
# binary rewritten in place (it kills the next launch of one). With a PATH dir,
# <dir>/pij-rs is linked to the copy. Prints the copy's path.
#
# `just rs-install`, `just bounce-rs` and `pij-rs daemon bounce` all install
# through this one script.
set -eu
if [ $# -lt 1 ] || [ $# -gt 2 ]; then
    echo "usage: install-pij-rs.sh <built pij-rs> [<PATH dir>]" >&2
    exit 2
fi
artifact=$1
if [ ! -f "$artifact" ] || [ ! -x "$artifact" ]; then
    echo "install-pij-rs: no built pij-rs at $artifact" >&2
    exit 1
fi
dir="$HOME/.local/share/pij/bin"
mkdir -p "$dir"
tmp="$dir/.pij-rs.$$.tmp"
trap 'rm -f "$tmp"' EXIT
cp "$artifact" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$dir/pij-rs"
trap - EXIT
if [ $# -eq 2 ]; then
    mkdir -p "$2"
    ln -sfn "$dir/pij-rs" "$2/pij-rs"
fi
echo "$dir/pij-rs"
