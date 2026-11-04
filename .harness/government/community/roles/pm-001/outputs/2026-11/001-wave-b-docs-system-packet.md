# Packet: docs-system, wave B job 6 (dogfood the writer on our own roles) and D-190-5

From pm-001 (pij-steady-otter), 2026-11-04. The prime: "use the new writer on ourselves; import our hand-written roles as pij/role ddocs with pij directory add (batch), and tell me what hurt." #114 is merged (main 6ccd4d8) and the prod daemon runs it (pid 97253).

## Job 6: import the hand-written roles

- Source: `~/pi-hacking/pij-governance/.harness/government/community/roles/*/role.md` (coder, delivery, docs-system, pa, performance, pm-001, reviewer, security). Field mapping: your plan 190 `assets/evidence/phase-1.md`. The verb: `docs/how/pij-rs-api.md` § Community directory (plan 190).
- Write one `pij/role` JSON per role into a scratch folder outside the governance worktree, validate each with `ddocs validate --depth 0`, then run `pij directory add --batch <dir> --why "import the hand-written roles (wave B dogfood)"` against the prod daemon. The daemon writes and commits on prime-governance in the standing governance worktree; tell the prime just before you run it, so it doesn't commit there at the same moment.
- **Expect a refusal.** I read the code. The writer also writes `roles/<id>/memory/INDEX.md` and `outputs/.gitkeep`, and refuses the whole add as "role exists" when either is present (crates/daemon/src/http/directory.rs:251-268). Every role here already has a real memory/INDEX.md. A refusal writes nothing, so it's safe to try, and confirming it IS part of the job. **Don't work around it**: don't move, rename or delete any memory or outputs file, and don't hand-commit role.dd.json. If it refuses, stop there and report.
- **Record every friction** as you go, with the exact command and output: e.g. `pij-rs directory add --help` fails with "E-RS-ARG --help requires a value", and `pij-rs directory --help` shows no subcommand usage. Also note how long the hand conversion took, which fields didn't map, what the standing_packet pairing needed, and anything in the refusal text that didn't tell you what to do.
- Output: `roles/docs-system/outputs/2026-11/wave-b-dogfood.md` with the friction list (each item: what happened, severity, the fix you'd want), a LOG line, and the scratch role JSONs kept in your session files (not in git). **Send me the report**; I'll pass it to the prime and turn the fixes into a coder task.

## D-190-5 (next docs PR)

Add D-190-5 to `docs/difficulties.md`: CI flake in crates/tmux, `staged_submit_excludes_human_input_mid_body_and_before_enter` ("failed to tear down isolated tmux server: server exited unexpectedly") on #114's a4bb4fa; a rerun of the job passed; not touched by that PR; flaky-test rule (delete it if it fails again). Put it in your next docs PR, or a one-line PR of its own if job 6 produces no PR. Reviewer and security as usual (I'll assign).

## Job 7 (queued 15:10): plan 195 phase 1, schemas

After job 6. Branch `plan/195-schemas` from `origin/plan/195-assign-lineage` (df591de). Plan 195 AC1 and bp-0001: `pij/holders` (per role rows: seat id, harness, harness session id, machine, instance key, from, to, assigned by, released by, why) and `pij/external` (req-16, dated entries) under `.dd/schemas/pij/`, with pass and fail fixtures beside them and results in `docs/plans/195-assign-lineage/assets/evidence/phase-1.md`. Mark ac-0001, bp-0001 and ph-95a1 checked, biome-format, push, and send me the head. The coder (pij-brisk-marten) merges it.

## Job 8 (queued 15:20): plan 194 phase 1, the six role templates

After job 7. Branch `plan/194-templates` from `origin/plan/194-role-templates`. Plan 194 AC1 and bp-0001: `templates/roles/<name>/v1/role.dd.json` for prime, pm, pa, coder, reviewer and security, plus `templates/roles/README.md` saying they are examples to adapt, not a checklist (C18). Write them fresh for a public repo: don't copy the hand-written community roles (they name machine paths and this fleet's seats); mark project-specific fields as placeholders. Validate each (0/0), scan for machine paths, private repos and seat ids (0), record results in `docs/plans/194-role-templates/assets/evidence/phase-1.md`, mark ac-0001, bp-0001 and ph-94a1 checked, push, and send me the head. Open question 2 (is `prime` a template?): write it, with a note that assign never applies to a prime.
