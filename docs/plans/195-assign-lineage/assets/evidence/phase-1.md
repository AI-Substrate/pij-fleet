# Plan 195 phase 1 evidence: pij/holders and pij/external schemas

Phase `ph-95a1`, criterion `ac-0001`, proof `bp-0001`. Written by pij-careful-heron (docs-system), 2026-11-04, ddocs 0.1.0, on branch `plan/195-schemas` from `plan/195-assign-lineage` (df591de).

## Schemas

| Schema | Path | Document it describes | Required |
|---|---|---|---|
| pij/holders | `.dd/schemas/pij/holders/schema.json` | `<government-root>/community/roles/<id>/holders.dd.json` (open question 1's recommended home) | `meta.role`; `rows[]` with `seat`, `harness`, `machine`, `from`, `assigned_by`, `why` |
| pij/external | `.dd/schemas/pij/external/schema.json` | `<government-root>/community/external.dd.json` (req-16) | `meta.project`; `entries[]` with `date`, `seat`, `from_project`, `note`, `by` |

`pij/holders` rows carry every AC1 field: `seat`, `harness`, `harness_session`, `machine`, `instance_key`, `from`, `to`, `assigned_by`, `released_by`, `why`. They add `release_why`, so a release or close keeps the assign reason and records its own (`"close"` for `pij close`, AC4), and an optional `id` so the writer can address a row with ddocs. An open row is one with no `to`. ddocs can't express "`to` and `released_by` come together", so the writer must keep that pairing (as with the standing packet in plan 190).

`pij/external` entries: `date`, `seat`, `from_project`, optional `role`, `note`, `by`, optional `id`.

## Fixtures: `ddocs validate --depth 0 --json <fixture>`

| Fixture | Expected | Actual (exit, status) |
|---|---|---|
| `holders/fixtures/holders-open-and-closed.dd.json` (a closed row, then an open one) | pass | 0, ok (0 error, 0 warn) |
| `holders/fixtures/holders-instance.dd.json` (a many-holder role, `instance_key`) | pass | 0, ok (0 error, 0 warn) |
| `holders/fixtures/holders-missing-from.dd.json` | fail | 1, error: `$.sections[rows].value[0].from: missing required field "from"` (1 error) |
| `external/fixtures/external-entry.dd.json` | pass | 0, ok (0 error, 0 warn) |
| `external/fixtures/external-missing-date.dd.json` | fail | 1, error: `$.sections[entries].value[0].date: missing required field "date"` (1 error) |

Instrument check: `from` set to the number 5 in a copy of the instance fixture gives `error: value must be a string`, so field types are checked as well as presence. `ddocs build` on the two main pass fixtures renders with no warnings (rendered files not committed). All fixture values are synthetic.

## Left for phase 2 (coder)

AC1 also says the writer's embedded schema copies (ruling C9) include the new schemas. Those copies are the `SCHEMAS` array of `include_bytes!` in `crates/daemon/src/community/layout.rs:33-46`, which is Rust and outside docs-system's scope. Phase 2 adds two entries, `("holders", …)` and `("external", …)`, and raises the array length from 3 to 5.
