# Workshop 003, input 001: what osk-central's seats learned from procedures

**Date**: 2026-11-23. **By**: pij-intense-herring, for the operator. **Feeds**: PRD 005 (procedures).

osk-central (another project the operator runs on pij) adopted procedures from 22 Nov, a few days before: ddocs with ids
`PROC-<area>-NNNN`, one folder each, steps that are `just` commands with precondition and done checks, a shared runner
and a progress ledger. Its prime (pij-ultimate-scallop) described the system and recommended a shape for pij. We then
interviewed 8 of its seats that had written, run, extended or reviewed procedures: rising-crane, select-crayfish,
subtle-butterfly, back-damselfly, unaware-platypus, relaxed-deer, smart-marlin and automatic-wombat. Seats that were cold
with large contexts were not woken. This is a summary; project-specific detail (its infrastructure, vendors and
security findings) is left out on purpose. The evidence is a few days of use by 8 seats: strong signals, not settled
practice.

## What worked (keep)

- **Steps are real commands with precondition, done and run, and they are idempotent.** "Already done" told a seat where
  it was after losing its context. A reviewer could check what a step does rather than what someone said they did. A gate
  test in one procedure made it impossible to ship stale output.
- **Refusals that name the fix**, such as "no graph in this folder: run step 0001".
- **One command books the id and scaffolds** the folder, the document, the justfile module and the import. Writing the
  document took about 20 minutes; the tooling around it took longer.
- **"Find the procedure, or write it as you go."** Three seats turned ad hoc work into a procedure step in the same PR.
- **Secret values never enter a seat's context** (copied from a file or a hidden prompt, never typed into chat).
- **Structured metadata** (id, status, owner, area) made a generated list of procedures about 15 lines. But the main
  procedures README is still hand-kept for the old flat pages beside it: the lesson is the two layouts, not the index.

## The biggest gap: preconditions that lived in one head (6 of 8 seats)

- Inputs and fallbacks were unstated: a step assumed two input files and was given one.
- Some preconditions lived only in infrastructure comments or in the author's memory: a login made in an earlier step,
  per-machine registered roots, which role may run a step.
- **Order across procedures** (create this before that, migrate before deploying the apps) was checked nowhere. Each miss
  cost about an hour.
- "Common mistakes" lists became "where real preconditions go to die". A precondition written as prose doesn't refuse.
- Asked for: steps that declare their inputs, named arguments (passed to the checks), fallbacks and the role allowed to
  run them; checked order across procedures; and, for steps that change access or secrets, a done check that reads the
  live state rather than the exit code of the change.

## Co-location

- **Held** for step scripts, generators, step tests, config, the procedure document and the human how-to.
- **Rightly outside, but linked**: what the procedure builds or operates, tests of a package's own code, shared runners,
  anything that handles secrets (keep one copy), and rules shared across procedures.
- **Hurt when outside and unlinked**: an ordering that lived in a script and one seat's memory, configuration with no
  procedure, a command in another tool.
- Side effects outside the folder (a line in a root file, the root justfile's import line, ignore rules) should be declared.

## Tooling friction to design out

- Commands and the document validator worked only from the repo root.
- The scaffold wrote into the main checkout after a failed `cd`; it should refuse outside a feature worktree and print
  where it writes.
- Step ids mixed numbered and named forms; picking the next number was manual; one step set had two names.
- A wrapper recipe silently dropped `--again`, so a step that should have rerun didn't.
- No easy way to add a step or set one field; prose sections were awkward through a structured editor.
- Ids must be unique across sections, which surprised authors.
- Precondition and done checks got no arguments.
- Trial runs stayed in the progress ledger for good; a "N operations have no procedure" warning on every run was noise.
- A changelog row wanting its PR number forced a second commit; generated renderings committed at a second head forced an
  extra review round.
- Long-running commands (serve, tunnel, watch) didn't fit the step model.
- Two PRs editing the same row on the same day silently dropped a clause.

## Don't copy

- **Two layouts** (flat pages beside procedure folders): three seats named this. Use folders only, from day one.
- Tooling that works from only one directory; flags swallowed silently; mixed step ids; a warning on every run.
- "Common mistakes" as the home of preconditions; security preconditions written as prose.
- Procedures that tell seats to save relayed messages into the repo (personal data ends up in git history).
- Data locations written into procedure text; cite them from one blueprint.

## Links

- A link to a PRD row or ruling helped when it carried a rule (a sentence saying what it guarantees), and was paperwork
  when it only named an id. Blueprint links were rarely read, and ignored under incident pressure.
- **Links between procedures** (a guide step pointing at a sub-procedure) were the most used.

## Candidates suggested by the "no procedure existed" answers

- A role reviewing a PR (its checklist, verdict format and record).
- Reproducing a failure seen in a deployed build.
- Recovery moves made under pressure, turned into commands.

## Questions for the workshop

See PRD 005's `open_questions` (q-01 to q-08).
