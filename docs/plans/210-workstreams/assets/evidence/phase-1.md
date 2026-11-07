# Plan 210 phase 1: evidence

docs-system (pij-leading-guan), 2026-11-08. Branch `plan/210-workstreams`, rebased onto main 8e19abc. All commands ran from the worktree root. No folder moved.

## tk-0001 schema

`.dd/schemas/pij/workstream/schema.json` is the roadmap schema plus `part_of` (a single link to `pij/workstream/section/steps`) and `lanes` (`branch` required; `worktree`, `stream`, `note` optional). `depends_on` targets `pij/workstream/section/steps`. Fixtures are in `.dd/schemas/pij/workstream/fixtures/`.

| fixture | `ddocs validate --json` |
|---|---|
| workstream-umbrella.dd.json | ok, 0 errors, 0 warnings |
| workstream-child.dd.json (cross-file `part_of` and `depends_on`, a lane with a stream) | ok, 0 errors, 0 warnings |
| workstream-lane-no-branch.dd.json | E402 `missing required field "branch"` |
| workstream-part-of-missing.dd.json | E430 `no entry with id "s09" in "steps"` |

- `ddocs links '…/workstream-umbrella.dd.json#steps/s02'` lists the child's two steps as inbound `rel: part_of`.
- A probe whose `part_of` pointed at roadmap 001 (`pij/roadmap`) failed E406: `link targets "pij/roadmap/section/steps", expected "pij/workstream/section/steps"`. Phase 2 must switch roadmap 001 to `pij/workstream` in the same commit as the move. The probe was removed.
- ddocs has no cross-row rule, so "an umbrella is complete only when its children are" can't be a schema check. It moves to the phase 2 coverage sensor (req-31).
- `pij/roadmap`'s description now says it is superseded. Roadmap 001 and the skill's example still validate 0/0.

## tk-0002 skill

- `skills/pij/references/community/workstream.md` (58 lines) is new; `roadmap.md` is a three-line pointer to it.
- The community README, prd.md, fold-a-ruling.md, `routes/community.md` (91 of 100 lines), `prime/protocol.md` (the lane line) and `prime/orient-oprime.md` follow.
- `bash harness/scripts/pij-skill-check.sh`: `✅ pij-skill-check: all green`.

## tk-0003 prd

- PRD 001: `ddocs set` on req-04, 18, 20, 21, 23, 24 and 27; `ddocs add` of req-30 (in-progress) and req-31 (planned); meta.updated; one changelog row.
- Blueprint 001: the overview, c-doc-schemas `where` and `responsibility`, req-30 added to its `realises`, one changelog row.
- Each was `biome format --write`, then `ddocs validate`: ok, 0 errors, 0 warnings. `ddocs build` regenerated the .dd.md.
- req-30 and req-31 are delivered by no step until phase 2 adds s16.

## tk-0004 templates

- v3 of coder, pa, pm, prime, reviewer and security.
- The structural diff from v2 to v3, field by field: "roadmap" becomes "workstream" in prose, `template_version` 3, and one changelog row. pm also gets one rule ("Shapes its workstream as the work demands: splits, merges or nests steps (`part_of`), and records where each step is built (`lanes`)") and a summary saying it runs one top-level workstream and nested ones with no PM of their own. pm's path `docs/project/roadmaps/<workstream number>-<slug>.dd.json` is unchanged.
- In a scratch repo, `ddocs validate --depth 0` gives 0 errors and 0 warnings on each v3. `biome check` is clean.
- `SHIPPED` and `PINNED` in `crates/daemon/src/community/templates.rs` go from 22 to 28 rows; `templates_http` expects coder versions `[1, 2, 3]`.
- The command ran niced, with CARGO_INCREMENTAL=0, on a shared target:
  `just shared-build -- test -p pij-daemon --lib community::templates --then test -p pij-daemon --test directory_http templates --then test -p pij-cli --test skill_verb_coverage`
  - community::templates: 8 passed, including `a_shipped_version_is_never_edited` and `every_template_file_is_embedded_byte_for_byte`.
  - directory_http templates: 7 passed, including `real_ddocs_validates_a_role_from_every_shipped_template` and `add_from_each_template_records_it_and_unknown_ones_refuse_by_name`.
  - skill_verb_coverage: 6 passed, including `the_community_route_and_templates_print_real_commands`.
- `cargo fmt -p pij-daemon --check`: clean.
