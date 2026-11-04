# Community directory (hand-written until s02)

Dogfood of PRD 001. Each role is `roles/<id>/role.md`, with `memory/` (one note per file plus `INDEX.md`) and `outputs/LOG.md`.

**Until the verbs exist:**
- **Assign:** the prime records holders in `HOLDERS.md`.
- **Outputs:** append one line to the role's `outputs/LOG.md`: `<date> <kind> <subject> <head> <result> — <notes>`. Verdicts are also posted on the PR as `<role>: pass|pass-with-notes|block (head <full-sha>)` followed by the notes.
- **Memory:** before any compact or close, write a note `memory/<yyyy-mm-dd>-<slug>.md` (title, tags, one-line hook, body) and add one line to `memory/INDEX.md`.
- **Numbers:** migration numbers are claimed from pm/001 before writing a migration.
- **Routing:** see `ROUTES.md`. Send owned work, don't do it.

Every seat commits only through the prime: send the prime the path and the prime commits here. Seats never commit to this branch themselves.

## Pending: external dogfood (Jordan, 2026-11-04)

When the tooling is done and we're happy with it, the osk-central prime (`pij-mutual-crane`) dogfoods it in its own repo:
1. After s02 merges and the daemon is restarted: we import our own hand-written roles with the writer first.
2. After s03 and s04 (wave B): crane sets up osk-central's directory from its existing briefs (`--from file:`).
3. After s06 and s07: crane moves its security verdicts and handovers to outputs and memory, and reports what works and what hurts.
