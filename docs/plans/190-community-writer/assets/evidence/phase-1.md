# Plan 190 phase 1 evidence: pij/role, pij/output, pij/memory schemas

Phase `ph-c762`, criteria `ac-0001` and `ac-0011`, proofs `bp-0001` and `bp-0008`. Written by pij-careful-heron (docs-system, ruling C7), 2026-11-04, ddocs 0.1.0.

## Schemas

| Schema | Path | Required sections |
|---|---|---|
| pij/role | `.dd/schemas/pij/role/schema.json` | meta (id, title, kind, lifecycle, holders), scope (rules), standing_packet (source), changelog |
| pij/output | `.dd/schemas/pij/output/schema.json` | meta (kind, role, subject, writer, date) |
| pij/memory | `.dd/schemas/pij/memory/schema.json` | meta (date, title, tags, hook), body |

AC1 field map:

- **pij/role**: `meta.{id, title, kind (specialist|general), lifecycle (long-lived|per-task), holders (one|many), instance_key, template, template_version, default_limit}`; `scope.{paths, rules}`; `out_of_scope`; `forbidden_paths`; `default_seat.{harness, model, effort}`; `inputs`; `output_kinds` (verdict|deliverable|note|handover); `standing_packet.{source (inline|brief), text, brief}`; `changelog[].{date, by, why}`. `default_limit` is a duration string (30m, 2h, 1h30m) for req-22's stall check when a send has no `--limit` (ruling C5).
- **pij/output**: `meta.{kind, role, subject, head, result, writer, writer_holds_role, requester, answers, link, date}`; `notes_owed[]`; `evidence[].{command, exit_code, log}`; `body`. `answers` is the request msg_id (ruling C3); `writer_holds_role: false` is the req-10 non-holder flag.
- **pij/memory**: `meta.{date, title, tags, hook}`; `body`.

ddocs has no union type, so "inline or by reference" is a required `standing_packet` section with a `source` discriminator. `brief` is a `target: "file"` link (repo-relative; a missing file is a WARN). The schema cannot require `text` when `source` is `inline` (or `brief` when it is `brief`); the writer must check that pairing.

## Fixtures: `ddocs validate --depth 0 --json <fixture>`

| Fixture | Expected | Actual (exit, status) | Notes |
|---|---|---|---|
| `role/fixtures/role-inline-packet.dd.json` | pass | 0, ok (0 error, 0 warn) | shaped like roles/security/role.md |
| `role/fixtures/role-brief-packet.dd.json` | pass | 0, ok (0 error, 0 warn) | shaped like roles/docs-system/role.md; packet in `role/fixtures/brief.md` |
| `role/fixtures/role-missing-scope.dd.json` | fail | 1, error: E402 `missing required section "scope"` (1 error) | |
| `role/fixtures/role-missing-packet.dd.json` | fail | 1, error: E402 `missing required section "standing_packet"` (1 error) | |
| `output/fixtures/verdict.dd.json` | pass | 0, ok (0 error, 0 warn) | head, result, notes owed, evidence, link, answers |
| `output/fixtures/deliverable.dd.json` | pass | 0, ok (0 error, 0 warn) | |
| `output/fixtures/note.dd.json` | pass | 0, ok (0 error, 0 warn) | |
| `output/fixtures/handover.dd.json` | pass | 0, ok (0 error, 0 warn) | |
| `memory/fixtures/note.dd.json` | pass | 0, ok (0 error, 0 warn) | |

Each fail fixture is the inline-packet fixture minus one section, so its single error is the one under test.

Instrument check: the brief fixture pointed at a missing `fixtures/nope.md` validates `degraded` with one WARN, so the file link is checked. `ddocs build` on the brief role, the verdict and the memory note renders with no warnings (rendered `.dd.md` files not committed).

Loop:

```bash
for f in .dd/schemas/pij/{role,output,memory}/fixtures/*.dd.json; do
  ddocs validate --depth 0 --json "$f" | jq -r '.status'; echo "exit=$?"
done
```

<a id="ac11"></a>

## AC11: req-26 seat naming (ruling C13)

PRD 001 gains req-26 (`docs/project/prds/001-pij-community.dd.json#requirements/req-26`) with a changelog row. State is `in-progress`, because part of it already exists: the daemon allocates `pij-<adjective>-<animal>` when `--id` is omitted (`crates/core/src/names.rs`; `NAME_NOUNS` in `name_corpus.rs` is animals and `SHIP_NAMES` is empty). The row's note names what is missing: an explicit `--id` is accepted as given, `--name` defaults the window to the seat id, and per-project role ids wait on s02/s04.

`ddocs validate --json docs/project/prds/001-pij-community.dd.json` → `ok`, 0 error, 0 warn (default depth).

## For the coder (AC6)

The two passing role fixtures are the shape for the eight-role batch: copy one per hand-written role under the governance `community/roles/`, moving the table into `meta`/`default_seat`/`output_kinds`, "Scope" into `scope.rules` (owned paths into `scope.paths`), "Not in scope" into `out_of_scope`, and "Standing packet" into `standing_packet.text`.
