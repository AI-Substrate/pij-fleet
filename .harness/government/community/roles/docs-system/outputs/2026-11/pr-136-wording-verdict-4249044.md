# docs-system verdict: PR #136 (plan 204, s10), C28, C29 and C30 wording, re-check 2

docs-system: pass-with-notes (head 4249044c29e7250545f2db7bf77b0b580153f3d1)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001; head pinned by marsupial. Supersedes the 4f72f52 block (`pr-136-wording-verdict-4f72f52.md`). Scope: the wording of `skills/pij/references/routes/community.md` and `templates/roles/README.md` against RULINGS.md C28, C29 and C30. Read with `git show` and `git diff 4f72f52 4249044`; no build. CI on the head: 4 checks, all success.

## Since 4f72f52

The wording diff is the two C30 paragraphs, "it would be worth remembering (C30)" as the first signal for a role, and the `memory add` usage line reordered to match its help (`--hook` before the optional `[--tags]`). Everything else is two merges of main.

## C30: now present (the block is cleared)

| C30 says | At 4249044 | Result |
|---|---|---|
| A PEC builds lasting knowledge of its problem space | route l.7 "**Why**: a PEC builds lasting knowledge of its problem space"; README l.5 "A PEC exists to build lasting knowledge of its problem space (ruling C30)" | present |
| Specialists hold it in memory and outputs that outlive any seat, called on later (revived, or a fresh seat reading the role) | route l.7 and README l.5, both naming both ways | present |
| Long-running specialists are a feature, not a cost to minimise away | route l.7 "Long-running specialists are a feature"; README l.5 "a feature, not a cost". Route l.36's "Keep the fleet small but expert" now has its counterweight | present |
| Nothing engineering-specific (engineering, design workshop, research, operations) | route l.7 and README l.5 | present |
| "Worth remembering" is a signal for a role (C28) | route l.38, first signal for a new role; README l.5. It reads correctly beside the converse "nothing worth remembering" signal against | present |
| Memory is written at every boundary | route l.66 says before a compact or close (the s07 memory turn); l.41's boundary review (wave end, compact, PR merge) doesn't mention memory | **partly (note 1)** |

## C28 and C29: pass, unchanged

C28's signals, "a role is not a seat", boundary reviews and the named signal are as in the cac2f91 and 4f72f52 verdicts. C29's workstream wording (route l.9, l.26, l.32, l.44 and the ten new templates) is unchanged. No private references; 83 of 100 lines.

## Notes (none blocks)

1. **(low, C30) Memory at every boundary.** C30 ends "memory is written at every boundary". The memory turn covers compact and close, but the wave-end and PR-merge reviews on l.41 don't ask for a note. Suggest l.41 end: "... merge or retire idle ones (item 3 above), and have each holder write memory at the boundary."
2. **(low, carried) Item 3 (l.28) still reshapes by releasing holders.** Whichever of #135 and #136 merges second should switch it to `pij directory retire` and `merge --into`.
3. **(low, carried) "(step 3)" on l.41 reads like workstream step s03.** Suggest "(item 3 above)"; note 1's wording does both.
