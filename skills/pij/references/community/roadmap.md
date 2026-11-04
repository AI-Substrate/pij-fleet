# Write a roadmap

A roadmap puts the work in order. Each step delivers requirement rows, builds components, names the roles it needs and points at the builder plan that does it. Read [README.md](README.md) first. Example: [examples/roadmaps/001-daily-digest.dd.json](examples/roadmaps/001-daily-digest.dd.json).

## Steps

1. Get the number from pm/001 and create `docs/project/roadmaps/NNN-<slug>.dd.json` with `"dd": {"schema": "pij/roadmap"}`.
2. Fill the sections. Required ones are marked *.
   - **meta*** `title`, `status`, `updated`, `owner`.
   - **goal*** one sentence.
   - **steps*** one row per builder plan:
     ```json
     {"id": "s01", "title": "...", "state": "planned",
      "delivers": ["../prds/NNN-<slug>.dd.json#requirements/req-01"],
      "builds": ["../blueprints/NNN-<slug>.dd.json#components/c-<name>"],
      "depends_on": [],
      "needs": ["role:coder", "role:reviewer"],
      "done_when": "...",
      "plan": "docs/plans/NNN-<slug>"}
     ```
     `depends_on` links steps in this file: `"#steps/s01"`. Leave `plan` out until the plan exists. `builds` may be `[]` until the blueprint exists, but write the blueprint before the step's plan starts. `state` is planned, in-progress, complete or abandoned.
   - **changelog**.
3. `git add -- <file>`, validate, build.

## needs

`needs` lists the roles a step can't finish without, as `role:<id>`, for example `role:coder`, `role:reviewer`, `role:security`, `role:devcontainer`. Name every role whose output the step's PR has to carry (a security verdict means `role:security`; a cost row means `role:performance`). The prime's view will flag a needed role that isn't in the directory and offer to create it. Until then, check the directory by hand: `harness/scripts/government-root.sh` prints `<government-root>`, and the roles are in `<government-root>/community/roles/`. (In a scratch clone it finds no governance worktree; ask the prime.) and tell the prime about a missing one.

## Checks before you hand it on

- Every requirement row is delivered by some step, and every component is built by some step. `ddocs graph --path docs/project` draws the joins.
- Steps that can run in parallel don't depend on each other. Steps that touch the same doc say in their `note` which merges first.
- Each step's `done_when` matches the requirement rows it delivers.
