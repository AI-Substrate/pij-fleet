# git commit --only refuses an untracked path

tags: governance-writer, git, plan-190
hook: For C8's writer, a new file needs `git add -- <path>` before `git commit --only -- <path>`.

Measured 2026-11-04 in a scratch repo: `git commit --only -m w -- r/new.json` on an untracked file exits 1 with "pathspec did not match any file(s) known to git". After `git add -- r/new.json` the same commit succeeds, leaves an unstaged edit to another tracked file and an untracked file alone. Folded into plan 190 AC4. Also: pm-001 commits need the trailers Pij-Seat, Pij-Prime, Pij-Plan and Co-Authored-By (harness/scripts/pij-commit-trailers.ts).
