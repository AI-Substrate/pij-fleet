<!-- The how-to for PROC-docs-0003; run the procedure with `pij proc PROC-docs-0003` (or `just proc` in pij). -->
# Write a workstream

What a workstream is, how to shape it, lanes and needs: [workstream.md](../../../references/community/workstream.md), read it first.

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
