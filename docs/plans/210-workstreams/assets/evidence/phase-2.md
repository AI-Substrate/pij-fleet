# Plan 210, phase 2: evidence

Coder: pij-following-marsupial (coder/210). Branched from main at 59120b3, after phase 1 (#159) merged.

## AC2: the rename

- `git mv docs/project/roadmaps docs/project/workstreams`, and the skill's worked example from `examples/roadmaps/` to `examples/workstreams/`. Both switch to `pij/workstream` in the same commit (a567520), because ddocs E406 refuses a `pij/workstream` link into a `pij/roadmap` doc. Each gets a changelog row and a "Workstream NNN" title.
- Live references follow: the PR template, the `docs/project` README, the skill README, `workstream.md` and the community route. The plan 204 canary rig copies `examples/workstreams` now.
- **Readers needed no change:** each already takes `workstreams` first and falls back to `roadmaps` (from #131 and #141):
  - the directory view's needs fold (`picture.rs`, `step_docs_read_workstreams_first_and_fall_back_to_roadmaps`);
  - claims' `workstream` kind and its `roadmap` alias (`claims.rs`);
  - `pij delivered`'s address resolution (`http/claims.rs`).
  Their `roadmaps` tests stay as alias coverage.
- **Left alone:** history in `docs/plans/` and `docs/retros/`, and the daemon's role fixtures (snapshots of the hand-written roles).

## AC6: the coverage sensor

- `harness/scripts/coverage-check.ts`, run by `just coverage-check`, a `coverage` stage in `.harness/extensions/checks/` and a step in `just self-check`. It fails, naming the row, when:
  - a PRD requirement is delivered by no workstream step (unless `dropped`);
  - a blueprint component is built by no step (unless `dropped`);
  - an umbrella step is `complete` while a `part_of` child is neither complete nor abandoned, across files (phase-1 tk-0005; ddocs can't express it);
  - a `part_of` names no step.
- **Known gaps:** listed in `harness/scripts/coverage-exemptions.json`, each with an owner: PRD 001 req-26 and PRD 002 req-01 to req-14, all for docs-system's AC4 follow-up. An exemption for a row now covered, or naming no row, fails, so the list only shrinks.
- **On this repo:** 0 findings with 15 exemptions. With none, it names exactly those 15, matching an independent python survey of the same docs.
- **Tests:** `harness/scripts/coverage-check.test.ts` covers link resolution, delivered, built and dropped rows, the umbrella across files, a missing parent, stale and unknown exemptions, and this repo. CI ran all 5 on Node 22 and 24 (run 42393139897). A local tsx probe of the same cases passed.
- **Cost:** reads three folders of JSON once per run. No loop, poll, launch or daemon change.

## AC7: governance re-point

[`ac7-repoint.sh`](ac7-repoint.sh) runs one `pij directory edit <role> --why 'C29 rename (plan 210)' --file <doc>` per live role whose document cites `docs/project/roadmaps/`. Only that path changes, and `edit --file` keeps each changelog and adds one row. The dry run on the governance branch head:

```
would edit coder: 1 line(s)
would edit delivery: 1 line(s)
would edit docs-system: 1 line(s)
would edit pa: 1 line(s)
would edit performance: 1 line(s)
would edit pm-001: 2 line(s)
would edit reviewer: 1 line(s)
would edit security: 1 line(s)
```

**Runs at merge:** it ran right after #160 merged (09aa310), so a prompt never pointed at a folder main lacked. The writer committed each edit on the governance branch, each with the `--why` and one changelog row:

| role | governance commit |
|---|---|
| coder | a93a486 |
| delivery | 8897e31 |
| docs-system | 5695f97 |
| pa | ebb9989 |
| performance | df3e8b5 |
| pm-001 | ec3dfc3 (also regenerated its ROUTES.md row) |
| reviewer | e32c8ec |
| security | 869ad9f |

Afterwards no `role.dd.json` and no `ROUTES.md` row in the governance tree cites `docs/project/roadmaps/` (bp-0007).

## Other items from phase 1's notes

- s16 is added (complete), delivering req-30 and req-31, which are now shipped. req-24's `delivered_by` is `#141, #159` and req-27's is `#136, #159`.
- pm template v4 has only the path change. v1 to v3 stay pinned; the pins now cover 29 versions.
- Four phase-1 task receipts now resolve from their folder (`../../evidence/`).
- **Known limit:** CI validates no ddocs until @ai-substrate/dd is on npm (F104, ruling C20); the step warns UNAVAILABLE on every run. `ci.yml` is untouched.

## Gates

- Local, macOS, niced on the fleet-194 target:
  - `clippy -p pij-daemon --all-targets -D warnings`: clean.
  - `pij-daemon --lib community::`: 93 passed.
  - `--test claims_http`: 8 passed. `--test directory_http`: 62 passed.
  - `pij-cli --test skill_verb_coverage`: 6 passed.
  - `ddocs-check`, `local-path-check`, `pij-skill-check`, `coverage-check` and repo-wide biome: pass.
- CI on b8823f3: all 5 checks green.

## Round 1 (verdicts at f886b8f)

Squirrel and bobcat both blocked on the coverage sensor; bobcat added two doc findings.

- **S160001 / F001, exemptions only shrink.** The 15 approved addresses are now a constant, `APPROVED_EXEMPTIONS`, in `harness/scripts/coverage-check.ts`. `coverage-exemptions.json` must be a subset of it, checked at runtime: each row needs a unique address and a non-empty owner and why, and an address outside the baseline fails ("exemption not in the approved baseline"). A rejected row is not applied, so its gap is reported as well. Removing a row is the only change the JSON allows; guan's AC4 PR deletes rows from both. Tests: a new real planned requirement with an exemption for it fails; a swap at the same count fails; a duplicate, an empty owner and an empty why fail; a removed, now-covered row passes; stale and unknown rows still fail.
- **Strict baseline (pm-001, after round 1):** a covered address still in `APPROVED_EXEMPTIONS` fails ("covered; remove it from the baseline"), so a closed gap can't be reopened by re-adding its JSON row. guan's AC4 PR deletes each covered row from both the constant and the JSON. Test: a covered baseline address fails, and the same row removed from both passes.
- **Same set (pm-001, with bobcat's finding):** `APPROVED_EXEMPTIONS` and `coverage-exemptions.json` must list the same addresses, each an existing row that is still uncovered. A baseline address whose row is deleted fails ("row missing; remove it from the baseline"), an address in the constant without a valid JSON row fails, and covered still fails. Tests: a deleted row plus deleted exemption, with the address left in the constant, fails; constant and JSON differing fails; the clean removal from the PRD, JSON and constant passes.
- **Duplicates (squirrel item 1 at 5021882):** before any coverage, exemption or umbrella work, a repeated required section and a repeated row id (per doc and section) are findings, so a second `requirements` section can't hide a new requirement, a duplicate id can't share an exemption, and a duplicate complete step can't mask an unfinished child. Tests build docs from direct arrays: a second requirements section fails, a duplicate requirement id fails, and a duplicate step id that would mask an unfinished child fails.
- **S160002 / F002, inputs fail closed.** A required folder (prds, blueprints, workstreams) that can't be listed, including ENOENT, fails, and so does one with no `.dd.json` documents. A doc or the exemptions file that can't be read or parsed fails, as does a missing or non-array required section or a row with no id. Only the legacy `roadmaps/` folder may be absent, and only on ENOENT; EACCES there fails. Reads go through an injected `Reader`, so the tests throw EACCES and EIO on `docs/project/blueprints` alongside the healthy and absent-alias positives.
- **F003.** `skills/pij/references/community/README.md` names `docs/project/workstreams/001-pij-community.dd.json` as the full-size example and in the Delivers example. A `rg --hidden` sweep found no other live instruction on the old path; the remaining hits are history (changelogs, workshops), shipped template bytes (pm v1 to v3), code that reads both folders, and alias tests.
- **N001.** req-18 and c-doc-schemas (with `ddocs set`, changelog rows, `ddocs build` and biome format), the community README and `templates/roles/README.md` now say the move is done.
- **Mutation check** (temp copy, vitest stub, no node_modules in the worktree): turning off the baseline check, the covered-baseline, row-missing and same-set checks, the repeated-section and duplicate-id checks, the error-to-empty fallback, the section check and the duplicate check each fail the tests written for them.
- Local: `coverage-check` (15 exempted), `ddocs-check`, `local-path-check`, `pij-skill-check` and repo-wide biome pass. vitest runs in CI (C25).

## Verdicts and states

- #160 merged at 09aa310. Final head 4a0bc62, CI green on all five checks, pr-freeze FINAL, one `Delivers:` line (s16, req-30, req-31).
- Verdicts at 4a0bc62, all pass-with-notes: reviewer bobcat 329c018, security squirrel c1216cf, performance ocelot ecbbf94 (governance commits).
- `pij delivered 160` printed `delivered_by #160` for req-30 and req-31, applied here. Its note asked to set s16's plan by hand because the PR touched plans 204 and 210; s16's plan already reads `docs/plans/210-workstreams`, so no change was needed.
- No new verbs or refusals, so `docs/how/pij-rs-api.md` and the skill's verb tables needed nothing.
- bp-0003 and bp-0005 are checked on phase 1's evidence (`pij-skill-check` green; `a_shipped_version_is_never_edited` and `every_template_file_is_embedded_byte_for_byte` passed), so they match AC3 and AC5.
- **Still open:** AC4, bp-0004 and phase 1 (ph-00d2). AC4's remainder is docs-system's: req-26's delivering step and PRD 002's 14 rows, which are the 15 rows in `harness/scripts/coverage-exemptions.json`. So the plan isn't closed. `harness plan validate --complete` also warns that ACs 2, 4 and 6 to 9 have no task that `satisfies` them, because phase 2 had no task file.
