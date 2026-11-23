# Procedures

How this project's repeatable work is done. One folder per procedure, `<area>/PROC-<area>-NNNN-<slug>/`, holds its
`procedure.dd.json`, the rendered `procedure.dd.md` (read this one) and anything only that procedure uses.

- List them: `just proc` (or `pij proc`). See a run: `just proc <id>`. Run a step: `just proc <id> <step>`.
- Write one: follow PROC-meta-0001 (`pij proc PROC-meta-0001`).
- Check them: `just procedures-check`.

No hand-kept index here: the list is generated from the folders.
