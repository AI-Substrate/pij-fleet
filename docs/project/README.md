# docs/project

What pij is for, and how it is built. Fleet governance (rulings, the community directory, role memory and outputs) lives on the governance branch instead.

```
docs/project/
  NNN-<slug>/
    prd.dd.json        what and why: requirement rows with state and done-when   (schema pij/prd)
    blueprint.dd.json  how it is built: components, decisions, costs             (schema pij/blueprint)
    workshops/         design sessions that fed the PRD
  roadmaps/
    NNN-<slug>.dd.json the order of work across PRDs                              (schema pij/roadmap)
```

- The joins are ddocs link cells: a roadmap step `delivers` requirement rows and `builds` components, and a component `realises` rows. `ddocs validate <file>` checks them.
- A builder sets a requirement's state in the PR that delivers it. Rows are amended, never deleted.
- Edit with `ddocs`, then `ddocs validate` and `ddocs build`. Never hand-edit a `.dd.md`.
- Each roadmap step names the builder plan in `docs/plans/` that does the work.
- No hand-kept index here; list the folders.
