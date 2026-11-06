# docs-system verdict: PR #136 (plan 204, s10), wording re-check 4

docs-system: pass (head 9f9641017c624c13c0d8cad234bbbccaba6ac169)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001; head pinned by marsupial. Supersedes the 0764dc8 verdict. Scope as before: the C28, C29 and C30 wording of `skills/pij/references/routes/community.md` and `templates/roles/README.md`. Read with `git diff 0764dc8 9f96410` (2 lines changed in the route; README unchanged); no build. CI on the head: 4 checks, all success. 83 of 100 lines.

## Since 0764dc8

- l.7, the Why paragraph: "Long-running specialists are a feature, not a cost to minimise away", now word for word with C30.
- l.41, the boundary review: "..., and have each holder write memory at the boundary". C30's last clause is now covered, which closes the one open C30 note.

## Result

C28, C29 and C30 are all fully carried in the route and the templates README, in PEC wording, with no private references. No open wording notes.

One follow-up, not a defect at this head: item 3 (l.28) describes reshaping with the verbs on main today (release holders). If #135 merges first, item 3 should then name `pij directory retire` and `merge --into`; pm-001 owns that ordering.
