#!/bin/sh
# Install a built pij-rs as a copy outside any cargo target/ (F187).
#
#   install-pij-rs.sh <built pij-rs> [<PATH dir>]
#
# The copy is $HOME/.local/share/pij/bin/pij-rs. Deleting or cleaning a
# target/ then never takes the CLI, or the daemon's next launch, down with it.
#
# A build identical to the installed copy leaves it untouched, so a daemon
# running it keeps a path it can execute (#229 review, P229-F001). Otherwise
# the build is copied into a temporary file made exclusively in the same
# directory (mktemp) and renamed into place: a daemon running the old copy
# keeps its own file, and macOS never sees a signed binary rewritten in place
# (it kills the next launch of one).
#
# The directories it creates under HOME are private (0700). It refuses to
# install through any of them, new or existing, that is not a directory owned
# by this user, or that is group- or world-writable (#229 review, S229-001).
#
# With a PATH dir, <dir>/pij-rs is linked to the copy. Prints the copy's path.
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
case ${HOME-} in
    /*) ;;
    *) echo "install-pij-rs: HOME must be an absolute path" >&2; exit 1 ;;
esac
if [ ! -d "$HOME" ]; then
    echo "install-pij-rs: HOME ($HOME) is not a directory" >&2
    exit 1
fi
uid=$(id -u)
dir=$HOME
for part in .local share pij bin; do
    dir="$dir/$part"
    mkdir -m 700 "$dir" 2>/dev/null || :
    # Judged after the mkdir, so one created by someone else in between is
    # judged too; a symlinked one is judged by the directory it leads to.
    if [ -z "$(find -H "$dir" -maxdepth 0 -type d -user "$uid" ! -perm -020 ! -perm -002 2>/dev/null)" ]; then
        echo "install-pij-rs: refusing $dir: it must be a directory you own that no one else can write to (chmod go-w it, or remove it)" >&2
        exit 1
    fi
done
if [ ! -f "$dir/pij-rs" ] || ! cmp -s "$artifact" "$dir/pij-rs"; then
    tmp=$(mktemp "$dir/.pij-rs.XXXXXX")
    trap 'rm -f "$tmp"' EXIT
    cp "$artifact" "$tmp"
    chmod 755 "$tmp"
    mv -f "$tmp" "$dir/pij-rs"
    trap - EXIT
fi
if [ $# -eq 2 ]; then
    mkdir -p "$2"
    ln -sfn "$dir/pij-rs" "$2/pij-rs"
fi
echo "$dir/pij-rs"
