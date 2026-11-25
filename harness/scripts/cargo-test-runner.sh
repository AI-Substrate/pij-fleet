#!/bin/sh
# cargo's target runner (.cargo/config.toml): cargo starts every test binary
# through this (incident #354). A seat that exported GIT_DIR/GIT_WORK_TREE ran
# the tests and their fixtures wrote into the real shared repository.
#
#   harness/scripts/cargo-test-runner.sh <binary> [args...]
#
# A test binary (cargo puts those under deps/) is refused while GIT_DIR or
# GIT_WORK_TREE is set, and starts without the rest of the inherited git
# location, config and namespace variables, so neither the test nor anything
# it launches (git, pij-rs, a script) can find a repository but its own.
# Anything else (`cargo run`) is executed untouched.
#
# Cost: none at steady state; builtins and one subshell fork per test binary,
# then exec (no program launched).

case "$1" in
*/deps/*) ;;
*) exec "$@" ;;
esac

set_vars=""
[ -n "${GIT_DIR+x}" ] && set_vars="GIT_DIR"
[ -n "${GIT_WORK_TREE+x}" ] && set_vars="${set_vars:+$set_vars and }GIT_WORK_TREE"
if [ -n "$set_vars" ]; then
	echo "cargo-test-runner: refusing to run $(basename "$1"): this environment sets $set_vars, so git in every test fixture would write into that repository (incident #354)." >&2
	echo "  fix: unset GIT_DIR GIT_WORK_TREE" >&2
	exit 2
fi

unset GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES GIT_NAMESPACE
# Every GIT_CONFIG* (GIT_CONFIG_GLOBAL, _COUNT, _KEY_n, _PARAMETERS, ...).
exported=$(export -p)
set -f
old_ifs=$IFS
IFS='
'
for line in $exported; do
	case "$line" in
	"export GIT_CONFIG"* | "declare -x GIT_CONFIG"*)
		name=${line#export }
		name=${name#declare -x }
		unset "${name%%=*}"
		;;
	esac
done
IFS=$old_ifs
set +f
exec "$@"
