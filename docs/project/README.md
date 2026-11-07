# docs/project

What pij is for, and how it is built. Fleet governance (rulings, the community directory, role memory and outputs) lives on the governance branch instead.

```
docs/project/
  prds/NNN-<slug>.dd.json        what and why: requirement rows with state and done-when   (schema pij/prd)
  blueprints/NNN-<slug>.dd.json  how it is built: components, decisions, costs; may serve many PRDs (pij/blueprint)
  workstreams/NNN-<slug>.dd.json the order of work across PRDs, nested and laned           (pij/workstream)
  workshops/NNN-<slug>/NNN-<topic>.md  design sessions that fed a PRD
```

Every file here is numbered. Take the next number with `pij claim <kind> --for <slug>` (PRD 001, req-20); it records your seat so parallel PMs never collide. Every PR body names what it delivers on a `Delivers:` line, and the merger runs `pij delivered <pr>` once after the merge (req-21).

- The joins are ddocs link cells: a workstream step `delivers` requirement rows and `builds` components, and a component `realises` rows. `ddocs validate <file>` checks them.
- A builder sets a requirement's state in the PR that delivers it. Rows are amended, never deleted.
- Edit with `ddocs`, then `ddocs validate` and `ddocs build`. Never hand-edit a `.dd.md`.
- Each workstream step names the builder plan in `docs/plans/` that does the work.
- No hand-kept index here; list the folders.

How to write each type, and fold a ruling in: [skills/pij/references/community/](../../skills/pij/references/community/README.md), with a small linked example set beside it.
