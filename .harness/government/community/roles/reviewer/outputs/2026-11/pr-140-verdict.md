# PR #140 final renumbering review

**pass** at `29c09d77ab9f391f0db868cef96f7f6e5cea1352`. Supersedes the `ebcdb10` verdict below. N001, the ledger-ID overlap with #138, is closed.

Reviewed only the one-commit renumber/removal delta after exact-head CI turned green:

- F17, F18 and F24 are removed here; #138 owns those rows.
- The retained fifteen items are F19-F23 and F25-F34, in F order, with consecutive IDs D-WB-18..32.
- Rows are moved to the table's end. Normalizing only the D-WB IDs and excluding the three intentionally removed rows produces an empty text diff against `ebcdb10`; severity, descriptions, owners and source references are unchanged.

An independent `git merge-tree --write-tree` with #138's exact `7c2b6b1` produces a clean `docs/difficulties.md` containing **32 rows, D-WB-1..32, with no duplicate IDs**. The resulting tree is `66dd0fdac0b6cd392d02570e105e88e5ca807981`.

Scope qualification: the full merge-tree exits 1 because of conflicts in three other files (`crates/daemon/src/http/directory.rs`, `docs/how/pij-rs-api.md`, `skills/pij/SKILL.md`). There is no conflict in the reviewed ledger. This docs-only verdict does not approve those unrelated resolutions or claim the entire pair of PRs merges cleanly.

Exact-head CI run [42158971078](https://github.com/AI-Substrate/pij-fleet/actions/runs/42158971078) passes check (22), check (24), rust-lint and rust-test. `gh pr view 140` confirms exact `29c09d7`, CLEAN, all four checks successful. Source review and publication waited for the successful CI result, as requested.

Validation used `git show 29c09d7`, an ID-normalized row-text comparison, and merge-tree/ID inspection. No local builds, test suites or new worktree; no branch, index or reviewer checkout was moved. No performance verdict needed; independent public-safety scan remains security-owned.

**magicWand fulfilled:** the revised rows occupy a non-overlapping allocation and separate merge hunk, preserving a unique combined ledger without changing the recorded evidence.

---

# Historical verdict at ebcdb10

**pass-with-notes** at `ebcdb101d4d73bbf0e32a27bedb2aa9992539f75`.

The change is exactly 18 added rows in `docs/difficulties.md`, D-WB-14 through D-WB-31, mapping consecutively to F17-F34. No source, schema or project-document changes. No correctness blocker in the frozen document.

## Source and owner checks

| Rows / items | Owner | Corroboration |
|---|---|---|
| D-WB-14/15, F17/18 | 208 | pm-001 LOG `holders-assigned-2`, the plan-206 split entry, and plan 208's recorded writer-lock difficulty |
| D-WB-16/17, F19/20 | 206 | `dogfood-s03.md:30-31`; pm-001 LOG's reshape/polish split |
| D-WB-18/19, F21/22 | 203 | `dogfood-s03.md:32-33` |
| D-WB-20, F23 | 204 | `dogfood-s03.md:34`; pm-001 LOG's plan-204 progress |
| D-WB-21, F24 | 208 | pm-001 LOG `warbler-target` correction |
| D-WB-22, F25 | Follow-up, no invented plan number | pm-001 LOG `critical-path slip` and `f25 - recorded` |
| D-WB-23/24/25, F26/27/28 | 209 | pm-001 LOG's skill-check, CI-flake, real-ps/QoS and plan-209 entries |
| D-WB-26, F29 | 210 | pm-001 LOG's second security assignment and F29/F33 owner entry |
| D-WB-27, F30 | 209 | `dogfood-s07.md:39`; pm-001 LOG's shim-stdin correction |
| D-WB-28/29, F31/32 | 209 | `dogfood-s07.md:34-35`; pm-001 LOG's s07 dogfood ownership |
| D-WB-30, F33 | 210 | `dogfood-s07.md:36`; its explicit F33 ownership exception |
| D-WB-31, F34 | 209 | `dogfood-s07.md:37`; pm-001 LOG's s07 dogfood entry |

Dogfood notes are under `<government-root>/community/roles/docs-system/outputs/2026-11/`; the log is under `roles/pm-001/outputs/LOG.md`.

Descriptions preserve the recorded symptoms and workarounds, including the native `pij-rs` versus shim stdin distinction. F25 correctly remains a follow-up rather than being assigned a fictitious plan.

Severities are sensible: one high for late priority/hold delivery affecting work and verdict validity; seven medium for blocked edits, hidden checks/flakes, QoS timing and memory write/date friction; ten low for presentation, naming and workflow polish. Explicit dogfood severities match.

## Merge qualification

**N001, convergence note:** still-open #138 also allocates D-WB-14..17. Its D-WB-16 and D-WB-17 represent F24 and F003, whereas this PR assigns those IDs to F19 and F20. Each frozen branch is internally coherent, but a combined ledger must not retain duplicate IDs or attach one ID to both meanings. pm-001 was notified to reconcile the overlapping document deltas at convergence. This verdict does not approve an unreviewed combined/conflict-resolved ledger.

## Validation and scope

Read the exact commit via `git show`, its one-file diff and GitHub file manifest. Compared all rows to the dogfood notes and PM records; no local tests, builds, probes or new worktree were required.

Exact-head CI run [42158420782](https://github.com/AI-Substrate/pij-fleet/actions/runs/42158420782) is green: check (22), check (24), rust-lint and rust-test. `gh pr view 140` confirms exact `ebcdb10`, CLEAN, all four checks successful. Comment publication waited for that green result.

No performance verdict is needed for this documentation-only change. The independent public-safety scan remains security-owned.

**magicWand:** preserve a single allocation of ledger IDs when parallel PRs document the same stream; factual row matching alone cannot prove convergence uniqueness.
