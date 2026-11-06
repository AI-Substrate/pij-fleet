# Write a blueprint

A blueprint says how the PRD is built: the components, the decisions behind them, and what each loop or launch costs. One blueprint may serve several PRDs. Read [README.md](README.md) first. Example: [examples/blueprints/001-daily-digest.dd.json](examples/blueprints/001-daily-digest.dd.json).

## Steps

1. Claim the number (`pij claim blueprint --for <slug>`) and create `docs/project/blueprints/NNN-<slug>.dd.json` with `"dd": {"schema": "pij/blueprint"}`.
2. Fill the sections. Required ones are marked *.
   - **meta*** `title`, `status`, `updated`, `owner`, `summary`.
   - **overview** one paragraph on the shape of the build.
   - **components*** one row per part someone will build:
     `{id: "c-<name>", name, kind, where, responsibility, realises: [...], state: "planned"}`.
     `kind` is one of verb, endpoint, store, file, schema, skill, template, hook, module. `where` is the path it will live at.
     `realises` links the requirement rows it serves: `"../prds/NNN-<slug>.dd.json#requirements/req-01"`.
   - **decisions** `{id: "d-1", decision, why, realises}` for every choice a reviewer might question.
   - **costs** `{id: "k-1", what, rate, scaling, component: ["#components/c-<name>"]}`. Required in practice for anything that runs on a timer, polls, hooks or launches a process (AGENTS.md: resource cost is a correctness property). `rate` is launches and frequency; `scaling` says whether it grows with seats, panes or time.
   - **changelog**.
3. `git add -- <file>`, validate, build.

## Checks before you hand it on

- Every requirement in the PRD is realised by at least one component. Check a row with the filtered `ddocs links` in [README.md](README.md#links).
- No cost row says "per tick per seat". If one would, change the design.
