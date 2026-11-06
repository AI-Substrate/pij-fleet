# PR #143 difficulty-ledger review

**pass** at `fdb0b2745c4a194227f0d405b99a089a4e92c39c`.

The change appends exactly twelve rows, D-WB-33..44 / F35-F46, to `docs/difficulties.md`. Existing rows are untouched. Descriptions, severity and ownership match the supplied source records; no correctness findings.

## Source checks

F35-F41 were checked against pm-001's supplied texts, received 6 November at 15:12 AEST, message `45a0ea93062f9320-000000000000006e`. The external report was relayed by the prime and has no separate source file; the reviewer requested this source rather than treating the ledger as its own corroboration.

| Item | Severity | Owner | Source symptom matched |
|---|---|---|---|
| F35 | medium | 209 | Add help omits required/either-or flags |
| F36 | medium | 209 | Invalid-ddoc next step directs a forbidden edit of a file not written |
| F37 | low | 209 | Missing sections reported one at a time |
| F38 | low | 209 | Chained adds commit individually before a later failure |
| F39 | medium | 203 | Five core-role prompts are noisy for a first team |
| F40 | medium | 209 | Dirty memory path blocks later writer calls until committed; refusal should identify paths and committer |
| F41 | low | 209 | Human delivered output omits edits returned in JSON |

F42-F46 match `<government-root>/community/roles/docs-system/outputs/2026-11/dogfood-s11.md:38-44`: raw shim claims JSON (medium), help stub (low), shim startup cost (medium), single-owner seeded claims (low), prose Delivers no-op (low). Ownership is 209 for F42-F44/F46; **F45 correctly records acceptance as-is**, not an unapproved implementation task or uniqueness failure. Its thirteen claims and never-reused-number explanation are consistent with the source.

The remaining wording retains the recorded timing values, workarounds and owner decisions. F39's F21/D-WB-20 cross-reference uses the corrected earlier ledger numbering.

## Combined ledger

Independent `git merge-tree --write-tree` with #138's exact `7c2b6b1` produces a conflict-free `docs/difficulties.md` with **44 unique D-WB IDs, consecutively 1..44**. Result tree: `82455d7bcbc94bd13a9aeeb21366e508fb7c75d3`.

The full merge-tree exits 1 for the same three unrelated source/API/skill files, not this ledger. This docs-only verdict does not approve their resolution or claim the entire pair of PRs merges cleanly. No branch, index or worktree was moved.

## Validation

Exact-head CI run [42166570116](https://github.com/AI-Substrate/pij-fleet/actions/runs/42166570116) succeeds for all four jobs: check (22), check (24), rust-lint and rust-test. Final GitHub check confirms exact `fdb0b27`, CLEAN, all checks green. Publication waited for CI success and the missing PM evidence.

Read-only git diff/source, supplied PM records, dogfood evidence and a document merge/ID check were used. No local builds, test suites or new worktree were required. Public-safety review remains security-owned; no performance verdict is needed.

**magicWand:** preserve relayed external F-item texts with the ledger review so a future reviewer can follow a source pointer rather than ask for the same messages again.
