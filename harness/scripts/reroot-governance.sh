#!/usr/bin/env bash
# Preparation only. The prime alone performs the printed, separate branch handoff.
set -euo pipefail
export GIT_OPTIONAL_LOCKS=0 GIT_NO_REPLACE_OBJECTS=1
unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE

mode=--dry-run
case ${1:-} in
  --dry-run|--prepare) mode=$1; shift ;;
  --help) printf 'Usage: reroot-governance.sh [--dry-run|--prepare] [REPO]\nDefault: read-only dry-run. --prepare creates backup and candidate refs only.\n'; exit 0 ;;
esac
if [[ $# -gt 1 || ${1:-} == --* ]]; then
  printf 'Usage: reroot-governance.sh [--dry-run|--prepare] [REPO]\n' >&2
  exit 2
fi
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo=$(git -C "${1:-.}" rev-parse --show-toplevel)
root=$("$script_dir/government-root.sh" "$repo")
standing=${root%/.harness/government}
source_ref=refs/heads/prime-governance
tag=prime-governance-pre-orphan-2026-09-07
candidate_ref=refs/heads/prime-governance-orphan-2026-09-07
source=$(git -C "$repo" rev-parse --verify "$source_ref^{commit}")
tree=$(git -C "$repo" rev-parse --verify "$source^{tree}")
message="governance: orphan re-root, history at tag $tag"

stop() { printf 'reroot-governance: %s\n' "$*" >&2; exit 1; }

check_clean() {
  local state path dirty
  dirty=$(git -C "$standing" status --porcelain=v1 --untracked-files=all --ignore-submodules=none) || stop 'cannot inspect standing worktree'
  [[ -z $dirty ]] || stop 'standing worktree is dirty; prime must commit or preserve all changes before proceeding'
  for state in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply sequencer BISECT_LOG; do
    path=$(git -C "$standing" rev-parse --path-format=absolute --git-path "$state")
    [[ ! -e $path ]] || stop "standing worktree has an operation in progress: $state"
  done
}

check_source() {
  local current_root current_branch current_tip
  current_root=$("$script_dir/government-root.sh" "$repo") || stop 'standing worktree registration changed'
  [[ $current_root == "$root" ]] || stop 'standing worktree path changed; rerun preparation'
  current_branch=$(git -C "$standing" symbolic-ref --quiet HEAD) || stop 'standing worktree detached during preparation'
  [[ $current_branch == "$source_ref" ]] || stop 'standing worktree branch changed'
  current_tip=$(git -C "$repo" rev-parse --verify "$source_ref^{commit}")
  [[ $current_tip == "$source" ]] || stop 'source tip changed; rerun preparation'
  if git -C "$repo" symbolic-ref --quiet "$source_ref" >/dev/null; then
    stop 'prime-governance must be a direct branch ref, not a symbolic ref'
  fi
  check_clean
}

check_unused_refs() {
  local ref
  for ref in "refs/tags/$tag" "$candidate_ref"; do
    if git -C "$repo" show-ref --verify --quiet "$ref"; then
      stop "ref already exists; inspect and preserve it, never overwrite: $ref"
    fi
  done
}

print_handoff() {
  printf '\nPRIME ONLY — pause every governance writer and hold exclusive ownership through handoff.\n'
  printf 'Run --prepare first if this was a dry-run. Then review and run this Bash block.\n'
  printf 'No push, reset, worktree removal, or automatic recovery is performed. If interrupted, STOP and inspect HEAD and refs.\n'
  printf '%s\n' '# BEGIN PRIME HANDOFF' '(' 'set -euo pipefail' 'export GIT_OPTIONAL_LOCKS=0 GIT_NO_REPLACE_OBJECTS=1' 'unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE'
  printf 'repo=%q\nstanding=%q\nsource=%q\ntree=%q\nsource_ref=%q\ncandidate_ref=%q\ntag=%q\n' "$repo" "$standing" "$source" "$tree" "$source_ref" "$candidate_ref" "$tag"
  declare -f stop check_clean
  if [[ -n ${candidate:-} ]]; then
    printf 'candidate=%q\n' "$candidate"
  else
    printf 'candidate=$(git -C "$repo" rev-parse --verify "$candidate_ref^{commit}")\n'
  fi
  cat <<'HANDOFF'
[[ $(git -C "$standing" rev-parse --path-format=absolute --git-common-dir) == $(git -C "$repo" rev-parse --path-format=absolute --git-common-dir) ]] || stop 'standing worktree repository changed'
[[ $(git -C "$standing" symbolic-ref --quiet HEAD) == "$source_ref" ]] || stop 'standing worktree is not on prime-governance'
[[ $(git -C "$repo" rev-parse --verify "$source_ref^{commit}") == "$source" ]] || stop 'source advanced; do not use this handoff'
[[ $(git -C "$repo" rev-parse --verify "refs/tags/$tag^{commit}") == "$source" ]] || stop 'backup tag changed'
[[ $(git -C "$repo" rev-parse --verify "$candidate_ref^{commit}") == "$candidate" ]] || stop 'candidate ref changed'
[[ $(git -C "$repo" rev-parse --verify "$candidate^{tree}") == "$tree" ]] || stop 'candidate tree differs'
[[ $(git -C "$repo" rev-list --parents -n 1 "$candidate") == "$candidate" ]] || stop 'candidate is not parentless'
check_clean
# Detach at the OLD tip, not the candidate: this cannot silently replace a different tree.
git -C "$standing" switch --detach "$source"
check_clean
if git -C "$standing" symbolic-ref --quiet HEAD >/dev/null; then stop 'expected detached HEAD'; fi
[[ $(git -C "$standing" rev-parse HEAD) == "$source" ]] || stop 'detached HEAD changed'
[[ $(git -C "$repo" rev-parse --verify "$source_ref^{commit}") == "$source" ]] || stop 'source advanced while detaching'
# Git refuses if another worktree has checked out prime-governance. Never bypass that guard.
git -C "$repo" branch -f prime-governance "$candidate"
check_clean
[[ $(git -C "$standing" rev-parse HEAD) == "$source" ]] || stop 'detached HEAD changed before reattachment'
[[ $(git -C "$repo" rev-parse --verify "$source_ref^{commit}") == "$candidate" ]] || stop 'source changed before reattachment'
git -C "$standing" switch prime-governance
[[ $(git -C "$standing" symbolic-ref --quiet HEAD) == "$source_ref" ]] || stop 'reattachment failed'
[[ $(git -C "$standing" rev-parse HEAD) == "$candidate" ]] || stop 'unexpected final tip'
check_clean
printf 'Re-root complete locally. Keep backup tag and candidate; remote publication is a separate prime decision.\n'
)
# END PRIME HANDOFF
HANDOFF
}

printf 'Mode: %s\nStanding worktree: %s\nSource tip: %s\nSource tree: %s\nBackup tag: %s -> %s\nCandidate orphan: %s\nRoot commit message: %s\n' "$mode" "$standing" "$source" "$tree" "$tag" "$source" "$candidate_ref" "$message"
printf 'Attribution command (invoked only by --prepare): (cd %q && pij-rs commit-trailers)\n' "$repo"
printf 'Preparation creates one parentless commit from the source TREE and atomically creates the two new refs, verifying the unchanged source tip.\n'

if [[ $mode == --dry-run ]]; then
  print_handoff
  check_source
  check_unused_refs
  printf '\nDry-run complete: no objects, refs, index, worktree, or attribution service were changed or invoked.\n'
  exit 0
fi

check_source
check_unused_refs
trailers=$(cd -- "$repo" && pij-rs commit-trailers) || stop 'pij-rs commit-trailers failed; no refs created'
[[ -n $trailers ]] || stop 'pij-rs commit-trailers returned no attribution; no refs created'
# Recheck after the external attribution call, before writing any Git object.
check_source
check_unused_refs
candidate=$(printf '%s\n\n%s\n' "$message" "$trailers" | git -C "$repo" commit-tree "$tree")
check_source
# Transactional create refuses collisions; verify protects against a concurrent source commit.
# A failure can leave an unreachable commit object, never a moved source or partial pair of refs.
printf 'start\nverify %s %s\ncreate refs/tags/%s %s\ncreate %s %s\nprepare\ncommit\n' "$source_ref" "$source" "$tag" "$source" "$candidate_ref" "$candidate" |
  git -C "$repo" update-ref --stdin || stop 'ref transaction refused; inspect concurrent changes; source was not moved by this script'
printf '\nPrepared candidate: %s\nStanding branch and worktree untouched.\n' "$candidate"
print_handoff
