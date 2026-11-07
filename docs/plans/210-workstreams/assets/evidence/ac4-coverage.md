# Plan 210 AC4, second half: coverage links (docs-system)

docs-system (pij-leading-guan), 2026-11-08. The branch is off main 09aa310, after phase 2 moved the folder to `docs/project/workstreams/`.

## What closes each gap

- **PRD 001 req-26**: workstream 001 gains s17 (planned), which delivers the row's open part: refusing an explicit `--id` that names a role or project. Code receipt for "still open": `crates/daemon/src/http/mod.rs:1426-1438` takes `request.id` as given and checks only whether the seat exists.
- **PRD 002 req-01 to req-14**: new blueprint 002 (claimed with `pij claim blueprint --for pij-base`) has 14 planned components, one per row, each at the code the row's note names. New workstream 002 (claimed with `pij claim workstream --for pij-base`) has 10 planned steps that deliver all 14 rows and build all 14 components. Each step `needs` only roles the pij-fleet directory has.
- No state changed. PRD 002's rows stay `planned` and were not re-triaged here.

## Exemptions

All 15 rows were removed from `harness/scripts/coverage-exemptions.json`, and all 15 addresses from `APPROVED_EXEMPTIONS` in `harness/scripts/coverage-check.ts`. The repo test "this repo is covered" asserted `exempted > 0`, which can't hold once the baseline is empty. It now asserts `exempted === APPROVED_EXEMPTIONS.length`, which holds at any size.

## Checks

- `tsx harness/scripts/coverage-check.ts` printed `every requirement and component is covered (0 exempted, each with an owner)`, with exit 0.
- `repoFindings(<repo>)` returned `{"findings":[],"exempted":0,"approved":0}`.
- Negative control, restored afterwards: with s17's `delivers` emptied and a stray blueprint 002 component added, `repoFindings` named both, `…req-26: no workstream step delivers this requirement` and `…c-probe: no workstream step builds this component`.
- `ddocs validate` gives 0 errors and 0 warnings on blueprint 002, workstream 002 and workstream 001. `ddocs-check` reports 9 documents clean with current renders. `local-path-check` and `biome check .` pass.
- The TS test file runs in CI only (C25: no node_modules in the worktree).
