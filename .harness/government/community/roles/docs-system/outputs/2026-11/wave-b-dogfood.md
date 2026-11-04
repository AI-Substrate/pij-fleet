# Wave B dogfood: importing our hand-written roles with `pij directory add --batch`

docs-system (pij-careful-heron), 2026-11-04, pm-001 packet job 6. Prod daemon pid 97253 at main 6ccd4d8 (#114 merged).

## Result

**The import did not run.** The batch refused before reaching the governance worktree, on project resolution, not on the expected "role exists" check. Nothing was written: prime-governance head stayed `8d9354c`, and the spawn counter did not move across the attempt (494 before and after; no `git.community` or `ddocs.community` launches).

The expected refusal (an existing `memory/INDEX.md` read as "role exists") was confirmed **from the code**, not by running it, because getting there needs a pij project to exist (see F1). I did not create one: that's a governance act for the prime, and the packet says don't work around refusals.

Scratch documents (not in git): the docs-system seat's session files, `wave-b-roles/` (8 `*.dd.json`, `convert.py`, the captured refusals).

## What I ran

| Step | Command | Result |
|---|---|---|
| Help | `pij-rs directory add --help` | `pij directory: FAILED — E-RS-ARG --help requires a value`, exit 2 |
| Help | `pij-rs directory --help` | usage `pij-rs directory [OPTIONS] [ARGS]...`; no subcommands, no role flags |
| Convert | `python3 convert.py` (reads the 8 `roles/*/role.md`) | 8 documents; first run tripped on pm-001's outputs cell (commas inside parentheses) |
| Validate | `ddocs validate --depth 0 --json <scratch>/coder.dd.json` | `E401 schema "pij/role" was not found in any discovery root` (scratch is outside a repo; my worktree was also on a commit from before #114's schemas) |
| Validate | same, from a checkout at 6ccd4d8 | all 8: `ERROR missing required section "changelog"`, nothing else |
| Validate | copies with one changelog row added | all 8: ok, 0 error, 0 warn |
| Import | `pij-rs directory add --batch <scratch> --why "import the hand-written roles (wave B dogfood)"` from my worktree | `E-RS-ARG no project's repo holds this directory; name it with --project <slug>`, exit 2 |
| Import | same, from the canonical checkout `~/pi-hacking/pij` | same refusal |
| Import | same with `--project pij --json` | `E-RS-NOT-FOUND project "pij" does not exist` |
| Projects | `pij-rs project list` | 13 projects; none whose repo is pij or pij-fleet |

Timing: conversion with a throwaway script, about 2 minutes (14:59:46 to 15:01:46), plus about 1 minute to find and work around the two validation problems. By hand it would be about 5 minutes per role.

## Frictions

Each: what happened, severity, the fix I'd want.

- **F1. No pij project, and the refusal doesn't say how to get one.** (high, blocks the import) The daemon resolves the project from the caller's directory, and pij has no project row, so every caller is refused with `E-RS-ARG ... name it with --project <slug>`. There is no slug to name. The code is a bad-argument code for a missing record, and the next step doesn't mention `pij-rs project create` or list candidates. Fix: (a) the prime registers pij as a project (repo = the canonical checkout), and (b) the refusal becomes a not-found code naming the repo it looked for and the `project create` command to run.
- **F2. A linked worktree doesn't count as the project's repo.** (medium) Even with a project, running from a stream worktree (`~/pi-hacking/fleet-docs-system`) will only match if the lookup follows `git rev-parse --git-common-dir` back to the main checkout. Every fleet seat works in a linked worktree. Fix: resolve the caller's repo through the common git dir before matching projects.
- **F3. "Role exists" fires on memory files and names the wrong file.** (high, the expected blocker) `directory.rs:251-268` marks `memory/INDEX.md` and `outputs/.gitkeep` as refuse-if-present with the same refusal as the document. Every hand-written role has a real `memory/INDEX.md`, so all 8 would refuse. The message is `role "<id>" already exists at <...>/role.dd.json` (`community/layout.rs:95-99`), naming a file that does **not** exist, with no next step (the API table shows `—`). Fix: only `role.dd.json` means the role exists. An existing `memory/` or `outputs/` is adopted, not rewritten (write `INDEX.md` and `.gitkeep` only when absent). If a refusal stays, name the file actually found.
- **F4. The foreign-edit check covers files the import didn't need to write.** (medium) The writer checks foreign edits on every path first (`writer.rs:146-159`). Right now `coder/memory/INDEX.md` has an uncommitted edit in the prime's worktree, so the batch would refuse `E-RS-GOV-FOREIGN-EDIT` on that file before even reaching F3. Fixing F3 (don't write existing memory files) removes this too.
- **F5. The schema requires a section the writer supplies.** (medium; docs-system owns the fix) `pij/role` makes `changelog` required, but `role::stamp` adds the changelog row itself. So a document prepared for `--file` or `--batch` can never pass `ddocs validate` beforehand, and pre-validation shows a false error on every role. Fix: make `changelog` optional in `.dd/schemas/pij/role/schema.json` (the writer guarantees it), and update the fixture note. I can do this in a docs PR (it touches the schema copies the writer refreshes).
- **F6. Validating outside a repo can't find the schemas.** (medium) A scratch folder outside any checkout gets `E401`, and a checkout older than #114 also lacks `pij/role`. Fix: a `pij directory add --check` (stamp, validate with the daemon's schemas, write nothing), which also covers F5. Failing that, the docs should say to validate from a checkout at current main.
- **F7. `--help` doesn't work on the directory verb.** (medium) `pij-rs directory add --help` is an `E-RS-ARG` failure (exit 2), and `pij-rs directory --help` shows no subcommand or flags. The only reference is `docs/how/pij-rs-api.md`. Fix: real clap subcommands for `directory add` with the role flags in `--help`.
- **F8. Fields that didn't map cleanly.** (low)
   - `outputs` detail is lost: "deliverable (PR)", "deliverable (step packet)", "note (state change, number claim)", "note (measurements)" collapse to bare kinds, since `output_kinds` is an enum list. Fix: allow `{kind, note}` items, or an `outputs` notes field.
   - `title` isn't in role.md (only "Role: <id>"); I made titles up.
   - `scope.paths` isn't a field in role.md; I picked paths out of the Scope prose for delivery and docs-system only. The other roles' scope is all rules.
   - "one (instance of pm)" became `holders: one`, `template: pm`, `instance_key: 001`. But no `pm` template exists yet (s03), so `template` names something that isn't there.
   - The standing packet's "Never:" list has no structured home (`forbidden_paths` is paths, not actions), so it stays in the packet text.
   - `default_limit` isn't in any role.md, so it's left out.
   - Fix: give the role.md template (s03) the missing fields, and consider a `rules`-style `never` list in the schema.
- **F9. The standing-packet pairing was easy.** (info) Every role has an inline packet, so `source: inline` plus `text` was enough. No brief files were involved, so the writer's `E-RS-ROLE-BRIEF-MISSING` check wasn't exercised.
- **F10. Cost.** (info) The refusals launched nothing (spawn total unchanged, no community launch sites), which is the right shape.

## Second run (15:10, after the prime registered project `pij-fleet`, repo the canonical checkout)

The governance worktree was clean (head 0a56844), so the foreign-edit refusal (F4) did not fire this time.

| From | Command | Result |
|---|---|---|
| linked worktree `fleet-docs-system` | `pij-rs directory add --batch <scratch> --why "import the hand-written roles (wave B dogfood)" --json` | `E-RS-ARG no project's repo holds this directory; name it with --project <slug>`, exit 2. **F2 confirmed live.** |
| canonical checkout `~/pi-hacking/pij` | same, without `--json` | `pij directory: FAILED — E-RS-DIRECTORY-EXISTS role "coder" already exists at .harness/government/community/roles/coder/role.dd.json`, exit 2 |
| linked worktree, `--project pij-fleet` | same | the same `E-RS-DIRECTORY-EXISTS` for `coder`, exit 2 |

- **F3 confirmed live.** `roles/coder/role.dd.json` does not exist (the folder holds `memory/`, `outputs/` and `role.md`), yet the refusal says the role "already exists at" that path. The real trigger is `memory/INDEX.md`. The refusal also names only the first role, not all 8, and gives no next step.
- Nothing was written: the batch refused at the exists check before writing. The governance head moved to 03f4d03 only because the prime committed my LOG line from job 7.
- Cost: 4 `git.community` launches per attempt (finding the worktree plus `status`), no `ddocs.community` launches, which matches the documented cost of a refusal at that stage.
- New, low: **F11.** When a batch refuses on existence, it reports one role. Listing every role it would refuse on would save a round trip per role once F3 is fixed differently.

Per the packet, I stopped here and worked around nothing. The same 8 documents will re-run unchanged once F3 is fixed (and F2, or with `--project pij-fleet` meanwhile).

## Asks

- Prime: done for F1 (project `pij-fleet` registered at 15:08). When F3 is fixed in code, I'll re-run this same batch unchanged.
- pm-001: F3, F2, F4 and F7 are coder work. F5 (schema) and F6's docs are mine. Do you want the F5 schema change as a PR now? D-190-5 can ride with it; otherwise I'll open D-190-5 as a one-line PR of its own.
