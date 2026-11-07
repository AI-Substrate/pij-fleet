# Write a workstream

A workstream puts PRD rows and blueprint pieces in an order humans and agents can follow (ruling C29). Each step delivers requirement rows, builds components, names the roles it needs and points at the builder plan that does it. Read [README.md](README.md) first. Example: [examples/workstreams/001-daily-digest.dd.json](examples/workstreams/001-daily-digest.dd.json) (written as a roadmap: the same steps, without `part_of` or `lanes`).

## Two words that never overlap

- A **workstream** is a planning doc (`pij/workstream`): what gets built, in what order, by which roles. It may nest, and maps to 0..n lanes.
- A **pij stream** is one lane of a workstream: a store record (`pij stream create`) holding one branch and one worktree. It orders nothing. A step names its streams in `lanes`; a stream never names a step.
- In a PEC project the workstream docs govern ordering. A prime-flow item that runs workstream work names its step and does not reorder it.
- A step id is `sNN` (`s10`), scoped to its file: write "workstream 001 s10" or the address. A stream's branch is `sNNN/<slug>` and its window `s<ordinal>-<slug>`. Don't use a bare `sNN` where a stream could be meant.

Workstream files live in `docs/project/workstreams/` (plan 210 moved them from `roadmaps/`). A `pij/workstream` doc can't link into a `pij/roadmap` one (ddocs E406), so nest only between `pij/workstream` docs.

## Steps

1. Claim the number (`pij claim workstream --for '<slug>'`; `roadmap` is its old name and claims the same numbers) and create `docs/project/workstreams/NNN-<slug>.dd.json` with `"dd": {"schema": "pij/workstream"}`.
2. Fill the sections. Required ones are marked *.
   - **meta*** `title` ("Workstream NNN: <name>"), `status`, `updated`, `owner`.
   - **goal*** one sentence.
   - **steps*** one row per builder plan:
     ```json
     {"id": "s02", "title": "...", "state": "planned",
      "delivers": ["../prds/NNN-<slug>.dd.json#requirements/req-01"],
      "builds": ["../blueprints/NNN-<slug>.dd.json#components/c-<name>"],
      "depends_on": ["#steps/s01"],
      "part_of": "NNN-<umbrella>.dd.json#steps/s04",
      "needs": ["role:coder", "role:reviewer"],
      "lanes": [{"branch": "coder/NNN-<slug>", "worktree": "fleet-NNN-<slug>", "stream": "<project>:<slug>"}],
      "done_when": "...",
      "plan": "docs/plans/NNN-<slug>"}
     ```
     Leave `plan` out until the plan exists. `builds` may be `[]` until the blueprint exists, but write the blueprint before the step's plan starts. `state` is planned, in-progress, complete or abandoned. Leave out `part_of`, `lanes` or `depends_on` when there are none.
   - **changelog**.
3. `git add -- <file>`, validate, build, format.

## Shape it as the work demands

A workstream is never fixed at its first draft. Shape, split, merge and nest it as the work changes (as C18, C26 and C28 say for roles), and add a changelog row each time.

- **Split** a step that needs two plans or two PRs: add new steps with new ids, then mark the old step `abandoned` with a note ("split into s14, s15"), or keep it as their umbrella.
- **Merge** steps that always ship together: keep one, move the other's `delivers` and `builds` onto it, and mark the other `abandoned` ("merged into s07").
- **Nest** a step that has grown its own ordered work: give that work its own workstream file, and point each of its steps at the umbrella with `part_of`. The child declares the parent; `ddocs links '<file>#steps/sNN'` lists the children. An umbrella is `complete` only when every child is. ddocs can't check that rule, so check it before you flip the umbrella.
- **Order across files**: `depends_on` may point at another workstream's step (`NNN-<slug>.dd.json#steps/s03`).
- **PMs**: each top-level workstream has a PM (`pm-<workstream number>`). A nested workstream is run by its umbrella's PM unless the prime gives it a PM of its own.

## Lanes

`lanes` says where a step is built: `branch` (required), `worktree` (its folder name, never an absolute path), and `stream` (`<project>:<slug>`) when a pij stream holds it. A step may have none (not started, or docs-only), or several (parallel lanes). PRs aren't stored here: `pij delivered` records them from each PR's `Delivers:` line.

## needs

`needs` lists the roles a step can't finish without, as `role:<id>`, for example `role:coder`, `role:reviewer`, `role:security`, `role:devcontainer`. Name every role whose output the step's PR has to carry (a security verdict means `role:security`; a cost row means `role:performance`). The prime's view flags a needed role that isn't in the directory and offers to create it. Until then, check the directory by hand: `harness/scripts/government-root.sh` prints `<government-root>`, and the roles are in `<government-root>/community/roles/`. In a scratch clone it finds no governance worktree, so ask the prime, and tell the prime about a missing one.

## Coverage, before you hand it on

- Every requirement row is delivered by some step, and every component is built by some step. `ddocs graph --path docs/project` draws the joins. Plan 210 makes this a `harness checks` sensor.
- Steps that can run in parallel don't depend on each other. Steps that touch the same doc say in their `note` which merges first.
- Each step's `done_when` matches the requirement rows it delivers.
