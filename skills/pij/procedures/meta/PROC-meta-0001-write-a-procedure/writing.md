<!-- The how-to for PROC-meta-0001; run the procedure with `pij proc PROC-meta-0001` (or `just proc` in pij). -->
# Write a procedure

A procedure is how one piece of repeatable work is done, written so any seat can do it again from the procedure alone.
Rule 0: before repeatable work, find the procedure and follow it; if there is none, write it as you go. A procedure lives
in one folder, `<home>/<area>/PROC-<area>-NNNN-<slug>/`, with everything only it uses: `procedure.dd.json`, its rendering
`procedure.dd.md`, its own `justfile`, step scripts, step tests, fixtures, templates and any extra how-to. Shared tooling
stays where it is and is listed in `uses`; files it changes outside its folder are listed in `side_effects`.

## Draft it

1. **Write from what really happened**, not from what should happen. The first run is the draft: copy the commands you
   actually ran, in order, into recipes.
2. **Draft from the main source, then mark the gaps.** When the job spans other roles' areas, draft the top level from the
   main document alone and list each gap with an id (`GAP-1`, …). Then send each specialist one message of lettered
   questions, (a), (b), …, each tied to a step and gap id, and ask for the answers in a file, not a message. Write the
   sub-procedures from those files.
3. **Split at about 10 steps.** A longer job becomes sub-procedures, reached from a guide step that follows each one.

## Write the steps

- **A run step is one `just` recipe**, in the procedure's own justfile (or the root justfile for a shared recipe). Step
  ids are four digits (`0001`), unique in the file and never reused.
- **A done check** (`done`) is a recipe that exits 0 only when the step's result is really there. With one, the step is
  idempotent: a second run says "already done". A step that changes access, permissions or secrets must have one, and it
  must read the live state, never the exit code of the command that made the change.
- **Preconditions are recipes that refuse.** Anything that must be true first is a `pre` check (on the step) or a
  procedure `precondition`, with a `fix` that says what to run first. Never write a precondition only in prose, or in
  `mistakes`: "common mistakes" explain, they never hold a precondition.
- **Inputs** are declared once with a description and a `fallback` (what to do without it), and listed on each step
  that needs them. Recipes read them as `PROC_IN_<NAME>`. A secret input says `secret: true`: it comes only from the
  environment, never the command line, and no step prints it.
- **Order across procedures:** when a step needs another procedure's result, `requires` links that step
  (`../../<area>/PROC-…/procedure.dd.json#steps/0002`). The runner runs that step's done check, so the result counts
  wherever it was made.
- **Decision steps** (`kind: decision`, `decide: <role>`) record a person's call; the runner refuses until it is recorded.
  **Guide steps** (`kind: guide`, `follow:`) point at a sub-procedure or a page (`./page.md` is in this folder).
- **Long-running commands** (serve, watch, a tunnel) go in `commands`, not `steps`: they run, and record nothing.
- Say **who** may run a step (`who`), and record where a step last ran for real in `proven_in_practice`.

## Pitfalls the first procedures hit

- Precondition and done recipes get no arguments: everything they need comes from `PROC_IN_*`, `PROC_KEY` and the
  other `PROC_*` variables (`PROC_ROOT`, `PROC_DIR`, `PROC_ID`, `PROC_STEP`, `PROC_RUNNER`).
- ddocs ids are unique across sections: a step `0001` and a mistake `0001` clash. Steps are `NNNN`, preconditions
  `p-N`, limits `l-N`, mistakes `m-N`.
- Links are relative to the document's own folder (`../../../../../docs/project/prds/…` from a skill procedure).
- Run `ddocs` from the repo root; the recipes do. From a subdirectory it can't find the schemas (E401).
- When the design changes under review, rewrite the steps and add one changelog row saying why.
- A run that finds a wrong step fixes the procedure in the same PR.
- Never tell a seat to save relayed messages or other raw input into the repo: personal data ends up in git history.
  The drift check refuses a procedure folder that holds a relayed pij message.

## Links

- `carries_out`: the PRD rows it carries out, each with a sentence saying what the procedure guarantees for it.
- `operates`: the blueprint components it operates (a note is optional).
- Procedure-to-procedure links (`requires`, guide steps) are the ones seats use most.

## Finish

Pass `pij proc check` (in pij: `just procedures-check`), commit the folder in your PR, and ask the operator for a yes.
A draft is usable as it is; approval is recorded with `pij proc status current <id> --approved-by <operator>`, and
several can be approved at once. A changelog row may say "this PR" instead of a number.
