# PR 125 correctness verdict

**reviewer: pass-with-notes (head 9384397762d2e7955ceff0de373640b85553227b)**

Reviewer: pij-interior-jellyfish. Date: 2026-11-06 AEST.
Base: db407b46fd1e85f2772397520305bb5e32252c83.
Scope: the complete eight-file docs-only diff, C18 and C26. No correctness blocker.
PR comment: https://github.com/AI-Substrate/pij-fleet/pull/125#issuecomment-6672125832

## Result

| Check | Evidence and result |
|---|---|
| Forward-only rename | PASS. All eight paths are modifications, with no rename, addition or deletion. Parsed comparisons preserve every prior id, requirement, component, decision, cost and roadmap step. Each document's old changelog is an unchanged prefix followed by exactly one C26 row. PRD 002 is unchanged. |
| req-28 | PASS, with the P3 below. Unique new id, planned/should, source C26, valid required fields. Retirement and merging with retained records are distinct from req-27's discovery and template prompts. The requirement has observable branch commits, changelog, readability and view checks, and does not claim delivery today. |
| Documents | PASS. PRD 001, blueprint 001, roadmap 001 and PRD 002 each validate at default depth 3 with zero errors and warnings; every generated sibling has drift:false. |
| Difficulty rows | PASS. D-WB-9 through D-WB-13 are the only ledger changes, in order. They match F12 through F16 in docs-system's wave-b-dogfood report, including its later-the-same-morning addendum: eight roles, commit 5a174e0, 14 old holder rows, UTC date mismatch, code-span escaping, absent historical import, and eight assign commits. D-WB-10's local-date comparison matches community README rule 4. F14 is correctly owned outside pij. |
| Publication | PASS for the changed content. No private owner/name identity, private issue content or host-state path is introduced. The governance report pointer uses <government-root>, not a machine path. Added-line home/host-state probes have positive controls. This is not a substitute for the separately owned security verdict. |
| Exact-head CI | PASS. Run 42127976303 reports headSha 9384397762d2e7955ceff0de373640b85553227b and conclusion success. check (22), check (24), rust-lint and rust-test all completed successfully. |

## Nonblocking note

**F001, P3: explicitly exercise preservation after merging as well as retirement.**
`docs/project/prds/001-pij-community.dd.json:383-387`

The statement protects a retired **or merged** role's memory, outputs and lineage, but the done_when's post-operation assertions name only "the retired role". A merge that commits successfully but loses one source role's records is not explicitly rejected by that acceptance sentence. When the planned merge/retire step is written, name the retained records of both merge sources and the merged/retired directory representation. The statement already supplies the preservation obligation, so this is acceptance-coverage tightening, not a blocker for recording a planned requirement.

No roadmap delivery step is demanded in this verdict; pm-001 owns that follow-up, as stated in the packet. Existing unrelated C22 reconciliation and role-input wording are unchanged and are not new findings here.

## Executed checks

| Exact command | Observed result |
|---|---|
| `ddocs validate docs/project/prds/001-pij-community.dd.json --json` | PASS, 0 errors / 0 warnings, depth 3 |
| `ddocs build docs/project/prds/001-pij-community.dd.json --check --json` | PASS, drift:false |
| `ddocs validate docs/project/blueprints/001-pij-community.dd.json --json` | PASS, 0 errors / 0 warnings, depth 3 |
| `ddocs build docs/project/blueprints/001-pij-community.dd.json --check --json` | PASS, drift:false |
| `ddocs validate docs/project/roadmaps/001-pij-community.dd.json --json` | PASS, 0 errors / 0 warnings, depth 3 |
| `ddocs build docs/project/roadmaps/001-pij-community.dd.json --check --json` | PASS, drift:false |
| `ddocs validate docs/project/prds/002-pij-base.dd.json --json` | PASS, 0 errors / 0 warnings, depth 3 |
| `ddocs build docs/project/prds/002-pij-base.dd.json --check --json` | PASS, drift:false |
| `git diff --check db407b46fd1e85f2772397520305bb5e32252c83...9384397762d2e7955ceff0de373640b85553227b` | PASS |
| `gh pr checks 125` | PASS, all four jobs |
| `gh run view 42127976303 --json headSha,status,conclusion,jobs` | PASS, exact head, completed/success |
| `harness boot` | NOT-READY advisory, absent local vitest; no downstream TS stages |
| `env -u FORCE_COLOR harness checks --quick` | FAIL, six dependency/tooling-limited stages; copilot-native PASS; attribution WARN; Rust and smoke deliberately skipped |

Quick-gate limitations: local-paths, ddocs and lockfile cannot launch the missing local tsx; typecheck/test lack worktree dependencies; lint resolves an incompatible Biome that rejects the existing includes configuration. No local full-gate success is claimed. C25 prohibits linking main's node_modules; exact-head CI supplies TS proof. Standalone ddocs validation/render checks above ran successfully despite the TS wrapper being unavailable.

## Guard controls

- In-memory comparisons reject a changed historic changelog row in each of the three renamed documents.
- A valid scratch pij/prd requirement validates clean; removing only done_when produces E402, exactly one error and no warnings.
- A generated scratch sibling passes build --check; changing only its heading produces exit 1 / E422 / drift:true; regeneration restores exit 0 / drift:false.
- Both injected forbidden-path examples are detected by the added-line path probe.

The render control initially received E429 outside a repository. It was rerun in a separate scratch git root with a copy of the exact-head PRD schema, never by writing into the review worktree. The first drift receipt parser wrongly expected success-shaped data on an error; the corrected parser asserts error.details.drift and E422. These were reviewer-fixture issues, not product failures.

## Harness feedback and handoff

magicWand: encode a read-only docs-review preflight that takes a pinned SHA, runs standalone ddocs without local TS dependencies, checks immutable old ids/changelog prefixes, and reports exact-head CI alongside dependency readiness. Repeated local wrapper failures should be a single NOT-READY diagnostic, not six misleading product failures. Route the encoding to the harness owner; no unrelated product changes are authorized by this review.

difficulties: local wrapper dependency readiness remains limited under C25; ddocs validation and rendering have different repository-root constraints; error evidence lives under error.details, not success data. The successful controls and constraints are retained here and in reviewer memory.

There is no changed runtime loop, poll, timer, hook, sweep, status line or launch site. No cargo, builds, dependency installation/linking, daemon operation, product edit, commit, push or checkout movement occurred. The review worktree remains clean at the pinned head.
