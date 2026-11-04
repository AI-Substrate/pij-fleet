# PR 118 review

**Verdict:** pass-with-notes
**Head:** `868e830398b3bcba4f1c6361a9920745723266b4`
**Base:** `6ccd4d8650fb5c41fcd747013e35819ed7aba8bb`
**Reviewer:** pij-legal-butterfly
**Date:** 2026-11-04

No blocking findings. The complete diff is 13 added lines in two Markdown files. The difficulty severities and routing are sensible; the writer does stamp, validate and render. Two low-priority wording qualifications follow.

## Findings

| ID | Priority | Location | Note |
|---|---|---|---|
| F001 | P3, non-blocking | `skills/pij/references/community/README.md:51`; `docs/difficulties.md:43` | "Always fails" applies to an unstamped input without a changelog, not every role document. The checked-in inline-packet fixture already has a changelog and validates with zero errors/warnings. `role::stamp` also accepts an existing changelog and appends a row. Keep the writer-owned workflow, but qualify the explanation as "unstamped role inputs without a changelog fail pre-validation." |
| F002 | P3, non-blocking | `docs/difficulties.md:44` | Schema discovery depends on the validation working directory, not simply the document's location. The same complete role document stored outside any checkout validates from this checkout's root, then fails E401 when validated from its scratch directory. Say "validation run outside a checkout with the schemas" rather than implying outside documents cannot be validated. |

Neither note requires a schema change or prevents the intended import workflow.

## Row-by-row evidence

Source report: `community/roles/docs-system/outputs/2026-11/wave-b-dogfood.md`, including its second run. Assignment evidence: pm-001's `outputs/LOG.md:86` routes F2/F3/F4/F7 to plan 195 and explicitly declines the F5 schema change; the docs-system packet also supplies D-190-5 and its flaky-test disposition.

| Row | Assessment |
|---|---|
| D-190-5 | Confirmed against CI run 41773746384 on a4bb4fa, attempt 1: the named adapter test panicked at `crates/tmux/tests/adapter.rs:2119` during isolated-server teardown with the recorded message. Attempt 2 passes all four jobs. The PR's base-to-head diff touches no `crates/tmux` paths. Medium severity and the PM's explicit repeat-failure policy are reasonable; no test is deleted here. |
| D-WB-1 | Matches report F1 and `http/directory.rs:334-390`: unnamed lookup returns E-RS-ARG; a missing explicitly named project returns E-RS-NOT-FOUND. The report's second run records the prime's project registration. High severity describes the original import blocker, not an unresolved registration task. Remaining diagnostics are assigned to plan 195. |
| D-WB-2 | Matches the live second run. The same resolver canonicalizes paths and performs a prefix comparison, without consulting the git common directory. Medium severity and plan 195 ownership are appropriate. |
| D-WB-3 | Matches live F3/F11. `http/directory.rs:250-268` attaches the same document-path refusal to the document, memory index and output placeholder. `community/writer.rs:160-167` returns the first existing-path refusal; `community/layout.rs:95-99` has no next step and formats the document path. High severity is justified because the community's existing roles cannot be imported. Proposed adoption belongs to plan 195. |
| D-WB-4 | Matches source-inspected F4, not a claimed live second-run failure. `community/writer.rs:134-159` includes all planned paths in the foreign-edit guard before the existence guard. Excluding already-existing memory from the planned writes resolves this without weakening protection of files actually written. Medium severity; coupled to D-WB-3 in plan 195. |
| D-WB-5 | The schema requires a changelog, and the writer supplies it before validation. Keeping req-04's schema contract and documenting the supported writer path matches the PM's disposition. Medium severity is reasonable; see F001 for the overly broad wording. |
| D-WB-6 | The E401 observation matches F6; embedded schema copies and writer-root validation avoid that discovery problem. Medium severity is reasonable; see F002 for the missing working-directory qualification. |
| D-WB-7 | Matches F7 and the parser: CLI `Directory` forwards trailing arguments (`crates/cli/src/main.rs:579-583`), while `http/directory.rs:90-96` demands a value after every non-json flag. A trailing help flag therefore produces the recorded error. Plan 195's ac-0010 owns real help. Medium severity is appropriate. |
| D-WB-8 | Faithful summary of report F8. `pij/role` has enumerated output kinds, required title, optional scope paths/default limit, and no separate action-prohibition section. Missing hand-written fields and invented conversion values are reported as conversion friction, not schema data loss. Low severity and roadmap s03/docs-system ownership are sensible. |

## Writer contract

`community/role.rs:133-177` creates the changelog section if absent, or appends to an existing one. `http/directory.rs:249` stamps each document before handing it to the writer. `community/writer.rs:182-191` validates then builds, using the writer's worktree; `community/layout.rs:20-45` supplies embedded schema copies. The README's operational instruction is therefore sound.

## Validation and boundaries

- `git diff --check <base> <head>`: pass. Detached HEAD stayed pinned and the review worktree stayed clean.
- `ddocs validate --depth 0 --json .dd/schemas/pij/role/fixtures/role-inline-packet.dd.json`: pass, 0 errors and 0 warnings.
- Scratch mutation removing only the changelog: expected E402, exactly one missing-changelog error. No source file was mutated.
- Complete scratch role: pass, 0/0 from the checkout root; expected E401 from the scratch working directory. These positive/negative controls establish F001/F002 rather than inferring them from source alone.
- Added-line privacy scan: zero matches across all 13 added lines, with seven positive controls and a permitted governance-pointer negative control. Manual diff review found no private identity, private content or machine-specific path.
- Exact-head CI: [run 41777732786](https://github.com/AI-Substrate/pij-fleet/actions/runs/41777732786), all four jobs successful: check (22), check (24), rust-lint and rust-test. GitHub still reported the exact requested head at final verification.
- `harness boot`: advisory NOT-READY because linked-worktree vitest is absent; downstream stages did not run.
- `env -u FORCE_COLOR harness checks --quick`: not green. Six local tooling stages fail (missing tsx/dependencies and incompatible ambient Biome); Copilot-native passes; Rust and smoke are skipped; attribution is unavailable. No install or dependency-link workaround was attempted. This is not a full local gate pass.
- No local build, cargo invocation, daemon mutation, runtime import, commit, push or checkout movement. This docs-only diff introduces no loop, poll, timer or launch site, so there is no changed steady-state cost.

## Harness feedback

**magicWand:** a read-only documentation-review preflight that pins the SHA, checks privacy and whitespace, returns exact-head CI, and reports local tooling readiness without suggesting forbidden installs.

**difficulties:** local composite proof remains tooling-limited, as in previous reviewer notes. In addition, `harness instructions boot` advertises `--full`, but the loaded command rejects it with E108; plain `harness boot` returns the dependency advisory correctly. These are harness follow-ups, not regressions in this Markdown-only PR.

**Suggested encoding:** keep the supported role-input and schema-discovery controls as a small documentation example check when the directory help/check surface is extended. Do not relax the changelog schema.
