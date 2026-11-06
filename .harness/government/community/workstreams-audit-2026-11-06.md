# Rename roadmaps to workstreams: read-only audit

Nothing was edited, built or tested. Evidence is from `grep -r` (which includes hidden dirs) over main, the `origin/*` refs for PRs #129–#138 (these match the GitHub PR heads), the governance worktree at `cea4b57`, and one live read each of `pij stream list` and `pij project list`.

## Summary

1. **Inventory.** On main, "roadmap" appears in about 70 files. Only about 25 of them are live surfaces: the schema, the roadmap doc, PRD 001, the docs README, 5 skill files and examples, 7 template files, plus test fixtures and comments. The rest are history (`docs/plans/190–195`, retros, difficulties). No hits in AGENTS.md, SKILL.md, `.omp`, `.copilot` or `.harness`, and no main code reads `docs/project/roadmaps`. The real code risk is in-flight: PR #131 (`pij claim`/`pij delivered`) hardcodes `roadmap` as a claim kind, the folder path, and the `pm-<roadmap>` lookup. Plan 203 (s09) will parse roadmap `needs` cells; it is coder branch only, with no PR yet.
2. **Model.** Steps are a flat list. There is no nesting and no worktree, branch or PR fields; the only work pointer is the `plan` string. One correction to the brief: **`needs` is roles (req-24), not ordering.** Ordering is `depends_on`, and its link type already allows steps in other files. Real link gaps:
   - req-26, req-27 and req-28 are delivered by no step. #135 adds s15 for req-28. req-27's note names s03/s09/s10 in prose, but no step links it.
   - **PRD 002 has 14 rows with no blueprint and no roadmap at all.**
   - 6 PRD 001 rows are realised by no component.
   - Every component is built by some step, and no links dangle.
3. **The stream clash is real, and it is worse than a word clash.** A pij stream is a store allocation: one project, one global ordinal, exactly one branch and one worktree (both unique), and a 1:1 fence. It has no parent and no nesting. The old o-prime protocol also uses "stream" for a plan-sized work item with its own orchestrator, stamped `--role pm` (the same "pm" word as the community PMs). pij-fleet itself has **0 streams** (live read); its worktrees are ad hoc (`fleet-NNN-*`).
   **Recommendation: option (a), unify loosely.** A workstream is the planning item. A pij stream is one worktree lane of a workstream, and the workstream doc lists stream ids. No store or CLI rename now, but reword the skill so the two words never mean the same thing.
4. **Prompting.** About 9 skill passages, 6 shipped templates and 8 live governance role prompts need new wording. Templates never change after they ship, so this means a v2 of all six. The 8 governance roles can't be rewritten until `pij directory edit` (#135) lands, because main's `directory add` refuses an existing role. Their prompts cite the roadmap file path, so they break as soon as the folder moves.
5. **Things you may not have thought of:**
   - After the folder rename, `pij claim workstream` would hand out **001 again**: an unknown kind has no folder to scan.
   - `pij delivered` would quietly tell no PM.
   - Comments in applied migrations 0031/0032 must not be edited. This is an inference: sqlx checks a checksum of each applied migration.
   - 7 of the 10 open PRs edit roadmap 001, so the rename should land after them.
   - Step ids `sNN` sit close to stream branches `sNNN/<slug>`.
   - A new ruling (C29) should say "rename forward" the way C26 did.

## Appendix A: inventory

**Main, live surfaces**
- Schema at `.dd/schemas/pij/roadmap/schema.json`. There is no `$id`; the id comes from the path, `pij/roadmap`.
  - `:3` description.
  - `:104` the `depends_on` link target `pij/roadmap/section/steps`.
  - Link names `delivers` (`:85`), `builds` and `depends_on` don't mention roadmaps.
  - The PRD schema's `delivered_by` is a plain string, not a link.
- The doc `docs/project/roadmaps/001-pij-community.dd.{json,md}`: `"schema": "pij/roadmap"` at `:3`, title "Roadmap 001" at `:9`, and s13's done_when.
- PRD 001 (`docs/project/prds/001-pij-community.dd.json`), by row:
  - req-04 `:141` ("for example a roadmap number")
  - req-18 `:281`, `:285`
  - req-20 `:301` (claim kinds)
  - req-23 `:321`, `:325`
  - req-24 `:330–331` (its title)
  - req-21 `:351`
  - req-27 done_when `:376`
  - req-28 note `:388`
  - q-02 `:405–406` (answered; history)
- Blueprint 001: `:18` (overview) and `:27` (`where: .dd/schemas/pij/{prd,blueprint,roadmap}`).
- `docs/project/README.md`: the layout block and lines 13 and 16.
- Workshops 001 (`:7`, `:70–71`, `:104–110`) and 002 (`:15`, `:19`). These are design history; leave them.
- Skills, under `skills/pij/references/community/`:
  - `roadmap.md:1,3,7` and the whole procedure
  - `README.md:1,9,12,20,21,23,36,44`
  - `prd.md:28`
  - `fold-a-ruling.md:3`
  - `examples/roadmaps/001-daily-digest.dd.{json,md}`
- Templates:
  - `templates/roles/pm/v1/role.dd.json:10,16,17,24,48,60`. The instance key is `<roadmap number>`.
  - `coder/v1:25,57`, plus pa, prime, reviewer and security v1 (their prompt line, "PRD, blueprint, roadmap"), and `prime/v1:32`.
  - `templates/roles/README.md:8,20`.
- Code, comments only:
  - `crates/daemon/src/community/{mod.rs:1,templates.rs:1}` and `http/directory.rs:1`
  - migrations `0031:1` and `0032:1`. Do not touch these (applied).
- Test data: `community/role.rs:484,494`, `tests/templates_http/mod.rs:93–112`, 8 fixtures in `crates/daemon/tests/fixtures/community-roles/*.dd.json` (copies of the governance roles), and `harness/scripts/ddocs-check.test.ts:55–79`.
- Schema fixtures: `.dd/schemas/pij/output/fixtures/note.dd.json:11` (`roadmap:001#steps/s02`) and `role/fixtures/role-brief-packet.dd.json:26`.
- The ddocs sensor walks `docs/project` recursively (`harness/scripts/ddocs-check.ts:8`), so it doesn't care what the folder is called.

**Open PRs only (`origin/plan/*`)**
- **#131** (196, claims):
  - `crates/core/src/claims.rs:19` `("roadmap",3)`, `:45` and `:53` (help text), `:88` (folder `docs/project/roadmaps`), `:185–189` `Address::roadmap()`.
  - `crates/daemon/src/http/claims.rs:345,390,437–447` (`pm-<roadmap>` lookup).
  - `docs/how/pij-rs-api.md:1025–1058`.
  - Skill `README.md:21,45` and `roadmap.md:7`.
  - Tests `claims_http.rs:28,110,326,378,484` and `store/tests/claims.rs:217`.
- **#136** (204):
  - `skills/pij/references/routes/community.md:22,28,30` ("PM: staff the roadmap's work").
  - 10 new templates, including `docs-keeper:33` ("PRDs, roadmaps") and a prompt line in the others.
  - These are unshipped, so they can be fixed before merge without a v2.
- **#129, #130, #132, #137** add only comments in Rust and migrations (`role_send.rs:1`, `routes.rs:1`, `output.rs:1`, migrations 0034–0036).
- **Plan 203** (coder branch `coder/203-prime-prompts`, no PR yet): plan AC "Code to read first: … roadmap docs (needs cells)" and "needs-based prompts". This is the next code that would hardcode the folder.
- PR body `Delivers:` lines with roadmap addresses: #131 (`…#steps/s11`) and #135 (`…#steps/s15`).

**Governance worktree (`cea4b57`, now through C28)**
- `RULINGS.md:30` (C26).
- `roles/pm-001/role.dd.json:16,23`.
- All 8 `roles/*/role.dd.json` system prompts cite `docs/project/roadmaps/001-pij-community.dd.json`.
- `roles/coder/role.dd.json:21`.
- `README.md:5`.
- History you leave alone: the handover file, plus the memory/ and outputs/ folders.

**Searches that came back empty**
- "workstream" appears nowhere in the main tree (all dirs except `node_modules`, `target`, `.git`) or in the governance `.harness/`.
- "roadmap" has no hits in AGENTS.md, `skills/pij/SKILL.md`, `.omp`, `.copilot`, `.harness`, the justfile, or `crates/*/src` non-comment lines.

## Appendix B: model check

**What's there today**
- A step has id, title, state, `delivers`, `builds`, `depends_on`, `done_when`, `plan` (string), `note`, and `needs` (strings like `role:<id>`).
- Nothing records a parent or children, worktrees, branches, PRs or stream ids.

**What ddocs can do** (read from the ddocs source `the ddocs source, src/core/validate.ts:84,310`)
- A link target is "schema/section", not "this file". So `depends_on` can already point at another workstream doc's steps; "same file" in the README is only a convention.
- A whole-file link exists as `target: "file"`, but it is untyped and repo-relative, while every other link is document-relative.

**Suggested schema changes**
1. **Nesting.**
   - Add `part_of: link → pij/workstream/section/steps` on a step. Within one file it builds a tree; across files a child doc points at the parent's umbrella step.
   - The child declares the parent, like `realises`, so there is one writer. Containment is read back with `ddocs links`.
   - Add a rule for state roll-up: an umbrella step is `complete` only when all its children are.
2. **Lanes.** Add optional `lanes: [{branch, worktree?, stream?: "<project>:<slug>", pr?}]`. Better still, take PRs from the `deliveries` table (#131) instead of keeping them by hand.
3. **Ordering across levels.** Keep `depends_on`, typed to the renamed schema, and allow it across files. Leave `needs` as roles.
4. **Ids.** Keep the `steps` section and `sNN` ids. Renaming them breaks every address and every `Delivers:` line.

**Coverage gaps (from the doc data)**
- **Rows delivered by no step:** req-26, req-27, req-28. #135 adds s15 for req-28.
- **Rows realised by no component:** req-19, req-24, req-25, req-26, req-27, req-28.
- **Components built by no step:** none.
- **PRD 002:** 14 rows, all `planned`, with no blueprint and no roadmap.
- **s01** is complete but has no `plan` cell, though the README says each step names its plan.
- **`delivered_by`:** used on no PRD row.

## Appendix C: what a stream is today

**Store**
- `streams(id = project:slug, project, ordinal UNIQUE, slug, branch UNIQUE, worktree UNIQUE, base_ref, created_by, created_at)` (`crates/store/migrations/0004_orchestration.sql:28`), plus `state reserved|created|closed` (`0014_governance.sql:8–10`).
- `fences.stream UNIQUE` makes fences 1:1 with streams (`0014:59–66`).
- Type: `crates/core/src/orchestration/governance.rs:53`.
- Naming: branch `s{NNN}/{slug}` and worktree `s{NNN}-{slug}` (`crates/core/src/orchestration/inventory.rs:139–145`).

**Verbs and API**
- `pij stream create|list|show|close` and `pij fence set|show` (`skills/pij/references/routes/node.md:27–35`).
- `POST /v1/stream` (`docs/how/pij-rs-api.md:760,801`).
- `close` changes the record, not the worktree.

**Protocol**
- A stream orchestrator is "one per in-flight work item, one plan end-to-end" (`skills/pij/references/prime/protocol.md:13`).
- "items become streams (proposed→…→done|folded|dropped)" (`orient-oprime.md:25`). Those states live in prime-flow, not the store (`prime-flow.schema.json:5–6`).
- Kickoff spawns the orchestrator `--role pm` into the worktree (`rituals/kickoff.md:8,18`).

**Live data**
- 12 streams, all in robotics and wayfinder.
- Project `pij-fleet` exists with 0 streams. pij-fleet has no prime-flow file.

**Options**

| | Pros | Cons and cost |
|---|---|---|
| **(a) Unify loosely (recommended)**: workstream = planning item; pij stream = one worktree lane of it; the doc's `lanes[].stream` holds stream ids | No migration. Matches what you described (0..n worktrees). The doc stays the planning authority and the store stays the authority for branches and worktrees. | Skill rewording in about 24 files and 127 "stream" lines to say "lane of a workstream". The old "item becomes a stream" doctrine has to change to "item becomes a workstream with ≥0 streams". |
| (b) Keep separate, rename `pij stream` to `pij lane` | No near-synonym left | Table, API, CLI and about 43 Rust sites change, plus other projects' live rows. Large cost for a primitive pij-fleet doesn't use. |
| (b′) Keep both words with no link between them | Zero cost | "workstream" and "stream" end up meaning overlapping things. Primes will confuse them. |
| (c) A store-side `streams.workstream` column | The daemon can join the two | The doc and the store would both claim the link (two owners). Defer this unless a view needs it. |

Prime-flow work-items are a third ordering surface that overlaps workstreams. Say in the skill which one governs. Inferred: in a PEC project like pij-fleet, the workstream docs should.

## Appendix D: prompting changes

**Skill**
- Rename `roadmap.md` to `workstream.md`. Add sections for split, merge and nest (via `part_of`), for 0..n lanes, for the check that every PRD row is delivered by some workstream, and for a top-level workstream readable on its own.
- Reword the community `README.md` (layout, links, states, claim kinds), `prd.md:28` and `fold-a-ruling.md:3`.
- Rename the examples folder.
- Add one line to `prime/protocol.md` and `orient-oprime.md` defining stream as a lane of a workstream.

**Templates**
- Ship v2 of pm: "PM for workstream <n>", instance key `<workstream number>`, and say which level of nesting gets a PM.
- Ship v2 of coder ("one workstream step") and of pa, prime, reviewer and security (their prompt lines).
- Update `templates/roles/README.md:8,20`.
- Fix #136's 10 templates and `community.md` before merge.

**PRD 001**
- Amend req-04, req-18, req-20, req-21, req-23, req-24 and req-27, plus the req-28 note.
- Add new rows: nesting and lanes, and the coverage check.

**Roadmap 001**
- Amend s13's done_when ("PRD and workstream").
- s09 and s10 should own the prompts that say workstreams are shaped as the work demands, never railroaded. Echo C18/C26/C28, the way req-27 does for roles.

## Appendix E: other changes and risks

**#131 must change before it merges**
- Add `workstream` to `KNOWN_KINDS` and its folder.
- Otherwise `pij claim workstream` gets the default width with no folder scan and returns **001**, which collides with the existing file (`claims.rs:12–14,88`).
- `Address::roadmap()` and the `pm-<roadmap>` lookup would match nothing and quietly print "no PM was told" (`http/claims.rs:390`).

**Merge order**
- #129, #130, #131, #132, #133, #135 and #137 all edit `roadmaps/001-pij-community.dd.json` (state, plan, changelog, and the new s15).
- Git rename detection usually carries edits to a moved file. The generated `.dd.md` files and the changelog tails will conflict, though: those PRs already conflict with each other there.
- Land the rename after this batch, or rebase it last.

**Governance**
- The 8 live role prompts point at the old path. They can only be rewritten after #135 adds `directory edit`, since main refuses an existing role (`http/directory.rs:454`).
- Write a C29 ruling ("workstreams; rename forward; history not rewritten", following C26 at `RULINGS.md:30`).

**Leave as history**
- `docs/plans/190–195`, retros, the plan-191 canary evidence, migration comments, and reviewer/pm memory and outputs.

**Smaller items**
- `ddocs build` regenerates the "Roadmap 001" headings.
- Step ids `sNN` sit close to stream branches `sNNN/` and stream windows `s<ordinal>-` (`protocol.md:137`). Consider saying so in the skill, or using a `w` prefix for new workstream docs only.

**Encode, don't document**
- Make "every PRD row delivered by a workstream, every component built" a `harness checks` sensor rather than prose.
- It has no loop or poll, so it adds no per-tick cost: it runs once per check.