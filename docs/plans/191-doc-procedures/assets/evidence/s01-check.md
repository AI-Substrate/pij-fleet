# s01 done-when check (AC4), before the flip

Recorded by pij-careful-heron (docs-system), 2026-11-04, on plan/191-doc-procedures at a90d04c (main 79b535b plus the plan), ddocs 0.1.0, run from the repo root.

s01 done-when: "Schemas resolve; this PRD, its blueprint and this roadmap validate clean; `ddocs graph` draws the joins."

| Check | Command | Result |
|---|---|---|
| schemas resolve | `ddocs schema show pij/prd` (and `pij/blueprint`, `pij/roadmap`) | ok, each from `<gitroot>/.dd/schemas/pij/<name>/schema.json` |
| PRD validates | `ddocs validate --json docs/project/prds/001-pij-community.dd.json` | ok, 0 error, 0 warn, depth 3 |
| blueprint validates | `ddocs validate --json docs/project/blueprints/001-pij-community.dd.json` | ok, 0 error, 0 warn, depth 3 |
| roadmap validates | `ddocs validate --json docs/project/roadmaps/001-pij-community.dd.json` | ok, 0 error, 0 warn, depth 3 |
| graph draws the joins | `ddocs graph --path docs/project --no-json` (exit 0) | 3 nodes; edges: 29 delivers, 22 builds, 41 realises, 17 depends_on, 4 costs. Saved as [s01-graph.mmd](s01-graph.mmd) |

Verdict: s01's done-when holds, so s01 flips to `complete` in this PR.

Note: AC4 says "shipped" for s01 and s13, but the `pij/roadmap` step_state enum is planned, in-progress, complete, abandoned. Steps use `complete`; requirement rows and components use `shipped`.
