# docs-system verdict: PR #136 (plan 204, s10), the C28 wording in the community route

docs-system: pass-with-notes (head cac2f91a3b0f56e33e25ae495a76507a2d291518)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001. Scope: the wording of `skills/pij/references/routes/community.md` at that head, mainly § "When to recommend a role (C28)" (lines 30-38), against RULINGS.md C28. Read with `git show cac2f91:<path>`; no worktree, no build. CI on the head: 4 checks, all success. The PR is a draft.

## Checks against C28

| C28 says | Route at cac2f91 | Result |
|---|---|---|
| Recommend roles from need; keep the fleet small but expert | l.32 "Keep the fleet small but expert"; l.22 "Add a role when a need has no owner, not because a template exists" | present |
| Two truths: small contexts cheaper and sharper; an expert in its own seat beats a generalist | l.32, nearly verbatim | present |
| Prime and PMs recommend as the work reveals them | l.32 "The prime and PMs recommend roles as the work reveals them" | present (see note 3) |
| For: same system or paths across tasks | l.34 | present |
| For: knowledge re-derived (same finding twice) | l.34 "a finding is re-derived (the same one twice)" | present |
| For: judgment independent of the author (security, performance) | l.34 | present |
| For: a seat bloating with two domains | l.34 | present |
| For: a path or system needs one owner | l.34 | present |
| Against: one-off work | l.35 | present |
| Against: overlap (extend the existing role) | l.35 "overlap with an existing role (extend that one)" | present |
| Against: nothing worth remembering | l.35 | present |
| Against: it would mostly sit idle | l.35 | present |
| A role is not a seat: roles many and cold (memory, lineage, queue, revived on demand); live seats few | l.36 "Roles can be many and cold: their memory, lineage and queue wait, and a holder is assigned when work arrives. Live seats stay few." | present |
| Review at boundaries (wave end, compact, PR merge): split overloaded, merge or retire idle (s15) | l.37, all three boundaries and all three moves | present (see notes 1 and 2) |
| Each recommendation names its signal | l.38, carried in `--why`, with a worked example (`--why "same finding twice: pane reads in s07 and s09"`); the add lines (l.14-16) take `--why "<the need>"` | present |
| Templates are examples | l.5 (C18, "not a checklist ... or write a role no template describes"), l.22, l.32 | present |
| PEC wording | title "run a Pij Emergent Community (PEC)"; l.5 cites C26 | present |
| No private references | no machine paths, home paths, private owner/name, hostnames or private content; links are repo-relative | clean |
| 100-line budget | 79 lines | within |

## Notes (none blocks)

1. **(low, sequencing with #135) Item 3's reshape steps will go stale when s15 lands.** l.24 retires a role by "release its holders and say why" and merges by "add the merged one, assign it and release the old holders". That's right at this head (no `retire` or `merge` verb exists here), but under #135 those steps leave the old role `active`: it still satisfies a core-role prompt, and nothing refuses work sent to it. Whichever of #135 and #136 merges second should change item 3 to `pij directory retire <id> --why` and `pij directory merge <from> --into <to> --why`, and add both to the command block. l.37's boundary review points at item 3, so it inherits the fix.
2. **(low) "(step 3)" can be misread as roadmap step s03.** In a route full of roadmap step ids, l.37's "(step 3)" reads like s03 (the templates step). Suggest "(item 3 above)" or a link to the "Prime" list.
3. **(info) "worker and specialist" was dropped.** C28 says the prime and PMs recommend "worker and specialist roles"; l.32 says "roles". Nothing is lost in practice, since the For signals include specialist ones (independent judgment, one owner). Keep the pair if you want the route to echo the ruling.
4. **(info) "Revived on demand" became "a holder is assigned when work arrives".** That's a fair plain-language reading, and it fits "a role is not a seat" better than the ruling's shorthand. No change needed.

Out of scope: whether every verb the route names (`routes`, `memory`, `claim`, `outputs`, `delivered`, `templates --show`) exists at this head. That belongs to the code reviewer, not this wording verdict.
