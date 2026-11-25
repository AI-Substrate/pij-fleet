# Pod templates: procedures and seed text

A pod template (`pod-template.dd.json`, schema `pij/pod-template`, PRD 008) says what a pod is made of. PRD 010 lets it
also carry **procedures about the pod itself**, so how to stand a pod up, run it and close it travels with the template
wherever it is copied.

## What a template's procedures are

The template's `procedures` section names each procedure it uses:

```json
{"name": "procedures", "value": [
  {"id": "PROC-pod-0001", "from": "template", "use": "up", "describe": "create the run's branch and worktree"},
  {"id": "PROC-pod-0002", "from": "template", "use": "close", "describe": "push the run's branch and open its PR"},
  {"id": "PROC-docs-0004", "from": "repo", "describe": "fold a ruling into the PRD"}
]}
```

- `from: template` is the template's own procedure, in its folder under `procedures/<area>/PROC-<area>-NNNN-<slug>/`,
  in PRD 005's layout. It goes wherever the template goes.
- `from: repo` links a procedure in the repo's own procedures (or the ones shipped with the pij skill). A procedure
  about the work, true whoever runs it, stays in the repo and is linked.
- `use` is `up`, `run` or `close`, or left out. A template names at most one `close` procedure.

`pij pod plan` lists each procedure with whether it is there, and warns with the fix about one that is not: a
template copied into a repo that lacks a linked procedure says so. `pij proc list --template <path>` lists the same,
and `pij proc <id> [<step>] --template <path>` runs a template's procedure. Its own procedures come first, and one
with the same id as a repo procedure hides it.

## The close procedure runs before `pij pod down`

When a pod's template names a `close` procedure, `pij pod down <pod>` first runs it to its end in your process, with
key `pod-<pod>`, and then takes the pod down (the freeze, then each member). If it stops early (a decision to record,
a guide step to mark, a failed check), down closes nothing and prints where it stopped and the command to resume.
`--skip-close-procedure` goes on without it and says so in the pod's log. That stop is a **brake**: removing it only
closes more. The pod's log records whether the close procedure ran, was skipped, or did not run (down asked for
without the pij CLI).

## What pij leaves to the template

pij does not do everything a run needs, and it does not enforce what it leaves out. The template's procedures and
seed text carry it:

- **Branches and worktrees.** `pij pod up` creates neither. A template that wants a run on its own branch carries an
  `up` procedure that creates the branch and worktree, and passes the folder as the pod's `work-folder` parameter, so
  `pod down` records where it stands.
- **Honesty instructions.** pij walls nothing off between runs. A template whose seats, in training, must not read
  earlier runs says so in its slots' seed text, for example:

  ```json
  {"id": "coder", "role": "coder", "seed": "This is a training run: do not read earlier runs' branches, outputs or notes for this subject."}
  ```

Nothing launches in the background: reading a template's procedures is a few file reads, and a procedure's steps
launch only what they name, when you run them.
