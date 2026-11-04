# Write a PRD

A PRD says what to build and why, as requirement rows that each have a state and a test of done. It does not say how. Read [README.md](README.md) first for numbering, ddocs commands and states. Example: [examples/prds/001-daily-digest.dd.json](examples/prds/001-daily-digest.dd.json).

## Steps

1. Get the number from pm/001 and create `docs/project/prds/NNN-<slug>.dd.json` with `"dd": {"schema": "pij/prd"}`.
2. Fill the sections. Required ones are marked *.
   - **meta*** `title` ("PRD NNN: <name>"), `status` (`draft` until the operator accepts it), `updated`, `owner` (your PM role, e.g. `pm-001`, or your seat id if you hold no role yet), `summary` (two sentences).
   - **why*** the problem in plain words, with the incident or numbers that prompted it if there are any.
   - **actors** who needs it: `{id: "a-<name>", name, need}`.
   - **goals** and **non_goals**: `{id: "g-1", goal}` and `{id: "n-1", text}`. Non-goals stop scope creep; write the ones people will ask about.
   - **requirements*** one row per thing that can be shipped and tested on its own:
     `{id: "req-01", title, statement, actor, priority: must|should|could, state: "planned", done_when, source}`.
   - **open_questions** `{id: "q-01", question, default, status: open|answered, answer}`. Give each a default so work isn't blocked.
   - **changelog** `{date, by, change}`; the first row is "First draft."
3. `git add -- <file>`, then `ddocs validate <file>` until it says ok with 0 and 0, then `ddocs build <file>`.

## A good requirement row

- **statement**: what is true when it's done, from the actor's side. Name what the actor sees or runs (a verb, a file, a behaviour), not how it is built inside; internals go in the blueprint. One requirement, not a list.
- **done_when**: a check someone can run or watch, e.g. "two seats claiming at once get different numbers". Not "works well".
- **source**: where it came from, as short free text: a workshop file, a ruling id, an issue, or `brief from <seat>`.
- If the feature adds a loop, poll, timer, hook or launch, add a requirement that bounds its cost (see `req-03` in the example). The usual check is the daemon's spawn counter, `pij-rs --json ping | jq .data.spawns`, read before and after. The blueprint then states the rate.

## Then

A PRD on its own is not work. Write or update the blueprint ([blueprint.md](blueprint.md)) and the roadmap ([roadmap.md](roadmap.md)) so every requirement is realised by a component and delivered by a step.
