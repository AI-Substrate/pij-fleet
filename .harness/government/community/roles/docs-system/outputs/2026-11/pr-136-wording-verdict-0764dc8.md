# docs-system verdict: PR #136 (plan 204, s10), wording re-check 3

docs-system: pass-with-notes (head 0764dc8a09737af2773dfe7b71dc48e3f44dc70b)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001; head pinned by marsupial. Supersedes the 4249044 verdict. Scope as before: the C28, C29 and C30 wording of `skills/pij/references/routes/community.md` and `templates/roles/README.md`. Read with `git diff 4249044 0764dc8`; no build. CI on the head: 4 checks, all success. 83 of 100 lines.

## Since 4249044

- **Folded in:** "(step 3)" became "(item 3 above)" (l.41, note 3 of the 4249044 verdict), and C28's "worker and specialist roles" is restored (l.36, the cac2f91 info note).
- **Verb accuracy (outside this verdict's scope, read for wording only):** the view line, items 1, 2 and 6, the PA `memory-stale` bullet and the `pij output` line now describe the verbs as built. Item 2 now says the view names the prime's seat, which answers F21's wording side. Item 6 no longer promises `pij directory routes`. None of it changes the C28, C29 or C30 wording.
- `templates/roles/README.md`: unchanged.

C28, C29 and C30 pass as in the 4249044 verdict.

## Notes (none blocks)

The 4249044 notes, renumbered here to avoid confusion:

1. **(low, C30, still open) Memory at every boundary.** This was note 1 at 4249044, and it is not the item-3 note. C30 ends "memory is written at every boundary". l.41's boundary review (wave end, compact, PR merge) still doesn't ask for a note; only compact and close get the memory turn (l.66). Suggest ending l.41 with "..., and have each holder write memory at the boundary."
2. **(low, waits on pm-001) Item 3 (l.28) still reshapes by releasing holders.** This was note 2 at 4249044. Whichever of #135 and #136 merges second switches it to `pij directory retire` and `merge --into`.
