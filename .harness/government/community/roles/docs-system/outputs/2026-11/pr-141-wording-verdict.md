# docs-system verdict: PR #141 (plan 203, s09), C28, C29 and C30 wording

docs-system: pass-with-notes (head 47bd3b2a9cb2fe45d0311a34b04cfc6210d3aa9f)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001. Scope: wording only, against RULINGS.md C28, C29 (workstream) and C30, in `crates/daemon/src/community/picture.rs` (`Signal::says`, `notice()`, `call_on`, and `idle` for the retire line), `docs/how/pij-rs-api.md` § "Growing the community", PRD 001's `meta.summary` and req-29, and the skill's `directory` row (`skills/pij/SKILL.md:54`). Read in the review worktree at the head without moving it; no build. CI on the head: 4 checks, all success.

## Checks

| Check | Where | Result |
|---|---|---|
| Every signal for (C28's five) | `Signal::FOR` and `says()` (`picture.rs:90-98, 126-135`); API table; req-29 | present, worded as C28 ("knowledge is re-derived (the same finding twice)") |
| Worth remembering as a signal FOR (C30) | `Signal::WorthRemembering`, in `FOR`, says "this would be worth remembering: knowledge that should outlive the seat"; API table; req-29 | present |
| Every signal against (four) | `Signal::AGAINST`; API table; req-29 | present |
| A role is not a seat | `notice()` "roles can be many and cold (memory, lineage and queue, revived on demand), while live seats stay few, and a cold specialist with rich memory is someone to call on, not to retire"; `idle()` keeps a cold role with memory; API; req-29 | present, and enforced: `idle()` returns nothing for a role with notes |
| Boundary review (wave end, compact, PR merge: split, merge, retire) | `notice()`; API "`data.reviews` is the boundary review"; req-29 | present |
| The named signal | `Recommendation` can't be built without a `Signal` (`picture.rs:58-60`); every prompt and review carries `signal: {code, says}`; the human view prints it; req-29 done_when | present, and enforced by the type |
| Templates are examples | `notice()`; `Signal::Core` "its template is an example"; skill row | present |
| C30's goal | `notice()` opens with it; API paragraph; PRD summary now leads with it; skill row | present: lasting knowledge of the problem space, specialists in memory and outputs that outlive seats, called on later (revived or a fresh seat), a feature not a cost, any domain |
| Calling on a cold specialist (C30) | `call_on()` prints a role send for a cold role with memory | present (see note 3) |
| C29 "workstream" | `WORKSTREAMS`/`ROADMAPS` (`picture.rs:19-25`), `Signal::Needed` "a workstream step names this role in its needs", API table, skill row | present; the old `roadmaps` folder is still read, as C29 says |
| PEC wording; no private references | all four places; the added lines have no machine paths, private owner/name, hostnames or names | clean |

## Notes (none blocks)

1. **(low) C28's two truths are missing from the prompt and req-29.** C28 tells the prime to recommend "weighing two truths: small contexts are cheaper and sharper, and an expert in its own first-class seat beats a generalist carrying the topic". `notice()` and req-29 keep only the conclusion ("keep the fleet small but expert"), so the prime gets the rule without the reason. #136's route has both. Suggest, after "keep the fleet small but expert" in `notice()`: "small contexts are cheaper and sharper, and an expert in its own seat beats a generalist carrying the topic." Add the same to req-29's statement.
2. **(low) C30's "memory is written at every boundary" is missing.** `notice()`'s boundary review, the API paragraph and req-29 list split, merge and retire, but not writing memory. #136's route now ends its boundary line with "and have each holder write memory at the boundary". Suggest the same clause in `notice()` and req-29, so the prime's prompt and the route agree.
3. **(low) `call_on` prints a different send form from the docs.** It prints `pij send --to role:<id> --body "..." --project <p>`, which is `pij-rs send`'s flag form under the `pij` name. The skill row (this PR) and #136's route both teach `pij send role:<id>[/<instance>]`, and `pij send --help` shows only the positional `pij send <id>`. I didn't confirm whether the shim accepts `--to`. Either print `pij send role:<id> --body ...`, or `pij-rs send --to role:<id> ...`, so the printed line runs as written.
4. **(info) Small attributions.** `notice()`'s doc comment says "(rulings C26, C28)" but it now carries C30 too, and the API heading reads "Growing the community (ruling C28)" over a C30 paragraph. The PRD changelog row says the summary leads with "lasting, searchable knowledge", but the summary itself doesn't say "searchable" (C30: "hard, searchable knowledge of the domain"). Add "searchable" to the summary or drop it from the row.

## How I checked

Read at 47bd3b2 in the review worktree (HEAD not moved, nothing written): `picture.rs:19-150, 236-262, 650-725`; `pij-rs-api.md:1104-1143`; PRD 001 `meta.summary`, req-29 and the last changelog rows via `jq`; the skill's `directory` row via `git show HEAD:skills/pij/SKILL.md`. Usage check: `pij-rs send --help` (`--to <TO>`) and `pij send --help` (positional `<id>`). Privacy scan: the added lines of those four files.
