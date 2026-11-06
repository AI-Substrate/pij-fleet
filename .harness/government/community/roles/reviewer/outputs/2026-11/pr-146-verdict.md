# PR #146 difficulty-ledger review

**pass-with-notes** at `9f6282da6900749be27cc83ebc463c36b73cf263`.

The one-file change appends fourteen rows, D-WB-45..58 / F47-F60, with no edits to prior records. Symptoms and severities match the available PM records and s05 dogfood note. No correctness blocker; owner transfers below are explicitly non-blocking per the review instruction.

## Source checks

- F47 matches the PM LOG's modal stall, approximately 13:30-15:16, including high severity and an unnumbered follow-up.
- F48 matches the recorded orphaned cargo/gate-shell incident, stale compilation and plan-209 assignment.
- F49-F51 match the PM's tapir memory-turn summary: refused shim stdin in the prompt, JSON memory rendering and the "one note" wording item.
- F52-F60 match `<government-root>/community/roles/docs-system/outputs/2026-11/dogfood-s05.md`: looping one-holder advice, help/receipt/argument gaps, invisible drain outcome, window restore, raw add output, queue visibility/withdrawal and the single-option wake wording.

Severity is sensible and consistent with the source: F47 high; F48/F49/F52/F56/F59 medium; the other eight low. F60 is framed as a proposed follow-up, not a fabricated implementation result.

## N001: updated owner transfers

Five new cells retain their earlier owner rather than the newly assigned shim-forwarding plan:

| Row | Item | Written owner | Current owner |
|---|---|---|---|
| D-WB-48 | F50 | 209 | 213 |
| D-WB-51 | F53 | 212 | 213 |
| D-WB-52 | F54 | 212 | 213 |
| D-WB-53 | F55 | 212 | 213 |
| D-WB-56 | F58 | 212 | 213 |

These are a non-blocking ownership correction, as pm-001 directed. The other new owners are consistent: F47 follow-up; F48/F49/F51 → 209; F52/F56/F57/F59/F60 → 212. The F57 link to F17/plan 208 is retained.

## Allocation and evidence

The new IDs map consecutively to F47-F60 and do not duplicate the prior ledger. An independent merge-tree with #138's exact `7c2b6b1` produces a clean `docs/difficulties.md` with **58 rows and 58 unique D-WB IDs**; tree `52ea52773ed5aa70b92e62abc71932e586ee1b33`. Whole-tree conflicts outside the ledger are not approved by this docs-only review.

F61 is already recorded in the source dogfood note/PM LOG, but is **not part of this head**. Any authorised F61 follow-up requires only its delta and new exact-head CI to be checked; this verdict does not pre-approve an unseen push.

Exact-head CI run [42190024583](https://github.com/AI-Substrate/pij-fleet/actions/runs/42190024583) passes check (22), check (24), rust-lint and rust-test. GitHub still reports exact `9f6282d`, CLEAN, all four checks successful. Publication waited for green CI.

Validation used pinned git show/diff, PM records, dogfood evidence and a document-only merge/ID check. No local builds, test suites or new worktree. Public-safety scan remains security-owned; no performance verdict is needed.

**magicWand:** owner transfers should update the ledger alongside the new plan, so a factual dogfood record does not acquire a stale execution owner before it lands.
