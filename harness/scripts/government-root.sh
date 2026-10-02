#!/usr/bin/env bash
# Resolve only the registered, available prime-governance worktree; never main's stub.
set -euo pipefail
export GIT_OPTIONAL_LOCKS=0 GIT_NO_REPLACE_OBJECTS=1
unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE

if [[ $# -gt 1 || ${1:-} == --* ]]; then
  printf 'Usage: government-root.sh [REPO]\n' >&2
  exit 2
fi
repo=$(git -C "${1:-.}" rev-parse --show-toplevel) || exit 1
common=$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)

refuse() {
  printf 'government-root: %s\n' "$1" >&2
  printf 'Bootstrap: /pij prime — follow skills/pij/references/prime/rituals/bootstrap.md § 3 (restore an existing branch; never recreate it).\n' >&2
  if git -C "$repo" show-ref --verify --quiet refs/heads/prime-governance; then
    printf 'If no worktree is registered, choose an unused permanent path and run:\n  git -C %q worktree add <standing-worktree> prime-governance\n' "$repo" >&2
    printf 'For an unavailable registered worktree, restore its directory/mount first; do not prune or replace it blindly.\n' >&2
  fi
  exit 1
}

# NUL records preserve spaces, tabs, non-ASCII and newlines in registered paths.
git -C "$repo" worktree list --porcelain -z | (
  path= root= count=0
  while IFS= read -r -d '' field; do
    case "$field" in
      'worktree '*) path=${field#worktree } ;;
      'branch refs/heads/prime-governance') root=$path; count=$((count + 1)) ;;
    esac
  done
  [[ $count -eq 1 ]] || refuse "expected one standing prime-governance worktree; found $count"
  [[ -d $root && -e $root/.git ]] || refuse "standing worktree is unavailable: $root"
  actual_common=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir) || refuse "cannot read standing worktree: $root"
  [[ $actual_common == "$common" ]] || refuse "standing path belongs to a different repository: $root"
  branch=$(git -C "$root" symbolic-ref --quiet HEAD) || refuse "standing worktree is detached: $root"
  [[ $branch == refs/heads/prime-governance ]] || refuse "standing worktree changed branch: $root"
  [[ -d $root/.harness/government ]] || refuse "government directory is unavailable: $root/.harness/government"
  printf '%s/.harness/government\n' "$root"
)
