# Plan 194 phase 1 evidence: the six role templates

Phase `ph-94a1`, criterion `ac-0001`, proof `bp-0001`. Written by docs-system (pij-careful-heron), 2026-11-04, ddocs 0.1.0, on branch `plan/194-templates` from `plan/194-role-templates` (9f06bdf).

## Files

| Template | Path | Kind, lifecycle, holders | Placeholders |
|---|---|---|---|
| prime | `templates/roles/prime/v1/role.dd.json` | general, long-lived, one | `<project>` |
| pm | `templates/roles/pm/v1/role.dd.json` | general, long-lived, one | `<project>`, `<roadmap number>` (title, `instance_key`, scope, packet) |
| pa | `templates/roles/pa/v1/role.dd.json` | general, long-lived, one | `<project>` |
| coder | `templates/roles/coder/v1/role.dd.json` | general, per-task, many | `<project>`, `scope.paths: ["<the paths named in the step's packet>"]` |
| reviewer | `templates/roles/reviewer/v1/role.dd.json` | general, per-task, many | `<project>` |
| security | `templates/roles/security/v1/role.dd.json` | specialist, long-lived, one | `<project>` |

Plus `templates/roles/README.md`: examples to adapt, not a checklist (C18); the core set and the security prompt (C22); prime is designated and never assigned (C24, open question 2); placeholders; one folder per version, never edited after shipping.

Each template carries `meta.template` (its own name) and `meta.template_version` `"1"`, a `summary`, scope rules, out-of-scope, inputs, output kinds, a `default_seat` that names only a harness (`omp`, the fleet default; model and effort are picked when assigning), example `default_limit` values for the per-task and verdict roles, and an inline standing packet written fresh: generic reading order, card reports, explicit sends, memory before compaction, and a generic never-list. The hand-written community roles were not copied (they name machine paths and this fleet's seats). The changelog row is `by: docs-system` (a role, not a seat).

No rendered `.dd.md` files are included: AC3's test will fail on any file under `templates/roles/` that isn't embedded, and the writer builds the role's `.dd.md` when it copies a template.

## Validation: `ddocs validate --json templates/roles/<name>/v1/role.dd.json`

| Template | Result |
|---|---|
| coder | ok, 0 error, 0 warn |
| pa | ok, 0 error, 0 warn |
| pm | ok, 0 error, 0 warn |
| prime | ok, 0 error, 0 warn |
| reviewer | ok, 0 error, 0 warn |
| security | ok, 0 error, 0 warn |

## Public-safety scan over all 7 files under `templates/roles/`

| Pattern | Hits |
|---|---|
| machine path (`/Users/`, `/home/`, `~/`, `/private/`, `/tmp/`) | 0 |
| the private predecessor repo's owner/name (read at run time from its checkout, never stored) | 0 |
| seat id (`pij-<word>-<word>`) | 0 |
| PR or issue number (`#N`) | 0 |
| `github.com` | 0 |

Instrument check: the same patterns on a planted string ("seat pij-example-heron at /Users/someone/x and ~/y, see #12 on github.com") count 2 machine paths, 1 seat id, 1 issue number and 1 GitHub URL.
