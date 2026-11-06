# docs-system verdict: PR #135 (plan 206), the pij/role schema part

docs-system: pass-with-notes (head ca33d01fc96dfc442123725a4ca82ee8f396ed5a)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001 under C7. Scope: `.dd/schemas/pij/role/schema.json`, its fixtures `role-bad-state.dd.json` and `role-retired-merged.dd.json`, and the PRD 001 and roadmap 001 edits (s15, req-28). The PR is a draft and its code isn't final, so this verdict covers the schema and project docs only. Evidence: ddocs only, from a read-only look at the review worktree at that head plus scratch copies; no cargo (backoff). CI on that head: 4 checks, all success.

## Checks

| Check | Result |
|---|---|
| Backward-compatible with existing roles | **Pass.** `state` and `merged_into` are optional; `meta` still requires only `id`, `title`, `kind`, `lifecycle` and `holders`. All 8 committed `role.dd.json` files from the governance branch (coder, delivery, docs-system, pa, performance, pm-001, reviewer, security) validate `ok 0/0` against this head's schema. The code reads an absent state as `active` (`crates/daemon/src/community/role.rs:212`). |
| Bad fixture fails | **Pass.** `role-bad-state.dd.json` fails with `E407 enum-invalid` at `$.sections[meta].value.state`: `value "archived" is not in active, retired`. One error, for the right reason. |
| Good fixture passes | **Pass.** `role-retired-merged.dd.json` (`state: retired`, `merged_into: security`): `ok 0/0`. The four earlier fixtures behave as before (2 ok, 2 fail). |
| `merged_into` only when merged | **Holds in the verbs, not in the schema; see note 2.** There is no `merged` state: a merge is `state: retired` plus `merged_into`, which matches plan 206 (merge "retires `<from>` pointing at `<to>`"). `state: "merged"` fails E407. ddocs has no conditional rule, so a document with `merged_into` and no state validates `ok 0/0` (probe). |
| Project docs validate | **Pass.** PRD 001, PRD 002, blueprint 001 and roadmap 001 at this head: `ddocs validate` (full depth) `ok 0/0` each. |
| `.dd.md` rebuilt | **Pass.** `ddocs build` on all four in a scratch copy of the head changes nothing (`git status` clean), so the committed renders are current. |

## Notes (none blocks)

1. **(info) Two states, not three.** pm-001's brief said "active, retired, merged". The schema has `active | retired`, and a merged role is `retired` with `merged_into`, as plan 206's AC says. Right as built; anyone writing about it should say "retired, and merged into X".
2. **(low, schema description and writer) `merged_into` isn't tied to `retired`.** ddocs can't express "only when retired", so the writer must hold the pair. `retire` and `merge` do (`role.rs:229-231`), but `add --file` and `edit --file` write whatever validates. The view derives `merged_from` from `merged_into` regardless of state (`crates/daemon/src/community/directory.rs:233`), so an active role carrying `merged_into` would show as merged into another role. Fix: the writer refuses a document with `merged_into` unless `state` is `retired`, or whose `merged_into` names a role that doesn't exist. Add one sentence to the schema description ("merged_into appears only with state retired; the writer enforces it"). The test belongs in code, since no ddocs fixture can fail on it.
3. **(for the code review, outside this scope) `edit` doesn't refuse a retired role.** `refuse_retired` is called by retire, merge and assign (`directory.rs:545, 581, 740`), not by `edit`. Because `edit --file` replaces the whole document and keeps only the changelog (`role::keep_changelog`), an edit whose document omits `state` quietly makes a retired role active again, keeping a now-dangling `merged_into` if it carried one. Either refuse edits to a retired role or carry `state` and `merged_into` forward the way the changelog is.
4. **(low, PRD 001) req-28's note is stale.** It still says "there is no retire or merge verb yet, and no roadmap step delivers this row", while the row is now `in-progress` and s15 delivers it. Suggested note: "Plan 206 (s15) adds `pij directory retire` and `merge`; adding a role shipped with s02."
5. **(low, roadmap 001) s15's done_when promises a send refusal that s15 can't deliver.** "a send to it refuses by name" needs role sends, which are s05 (planned). The tests cover retire, assign and merge refusals (`crates/daemon/tests/reshape_http/mod.rs:357-372`), not a send. Either reword to "an assign, retire or merge naming it refuses by name, naming the target after a merge", or add `#steps/s05` to `depends_on` and leave the send clause for then.
6. **(low, roadmap and blueprint) Links are incomplete.** s15 has no `needs`, though it changes a docs-system schema (C7). Like s13 and s14, it should list `role:docs-system, role:reviewer, role:security`. And s15 `builds` `c-verbs`, but `c-verbs.realises` in blueprint 001 doesn't list req-28, and the blueprint has no changelog row. ddocs passes, because nothing requires the reverse link.

## How I checked

- Schema diff read at the head. Required fields read from `schema.json` (`meta.required`). Code for state and merge read at `role.rs:181-232`, `directory.rs:215-240, 545, 581, 612-660, 740, 1052-1060`.
- Scratch repo with `git archive ca33d01 .dd` plus the 8 governance `role.dd.json` files and all six fixtures; `ddocs validate --depth 0` on each. Probes: `merged_into` with no state (ok 0/0); `state: "merged"` (E407).
- Scratch repo with `git archive ca33d01 .dd docs/project`; `ddocs validate` and `ddocs build` on the four docs; `git status` clean afterwards.
- The review worktree's HEAD was not moved, and nothing was written there.
