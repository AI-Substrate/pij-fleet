# PR 126 correctness verdict

## Current verdict: repair review

**pass-with-notes** at `6b820d0e04c89a6c7980c5cbc94f8e5258baa6aa`. **F001 closed. No correctness changes owed.** This supersedes the block at 04144d1 retained below.

Current PR comment: https://github.com/AI-Substrate/pij-fleet/pull/126#issuecomment-6673180380

Reviewed the complete seven-file, single-commit delta from 04144d1 in the new detached `fleet-review-126b` worktree. The old worktree remains clean at 04144d1; the new one remains clean at the repaired head. Templates, schemas, requirement/state reconciliation and dependency manifests are unchanged; their original review evidence carries forward.

`DirectoryService::view` now passes its project to every core/security prompt, and both add commands and the native drift hint explicitly carry `--project`. Both variable words in the drift hint use the shared `pij_core::directory::shell_word`; prompt names are fixed constants and their project uses that helper too.

The original independent two-repository replay now passes. I parsed the exact printed command with the real `/bin/sh` into argv, then replayed that argv through the pinned HTTP service: `data.project` is remote, the remote security file exists, and the local one does not. This does not rely on the committed test's limited shell splitter.

| Repair proof | Result |
| --- | --- |
| `cargo test --locked -p pij-daemon --test directory_http templates -- --test-threads=1` | Six passes, including exact-prompt two-project regression, real-ddocs all-six writes, drift/no-mutation and default-project cases |
| Independent `pr126b-independent-replay review_explicit --nocapture --test-threads=1` | Two passes: original cross-project case with real-shell argv, plus distinct v1/latest content control |
| New committed two-project regression linked against original 04144d1 library artifacts | Expected failure: all printed prompts omit the project; proves the regression detects the original defect |
| `target/debug/deps/pij_rs-534f3dc7c5567452 tests::the_drift_hint_names_the_viewed_project --exact --nocapture` | One pass; quoted project preserved in the actual native renderer |
| Standalone tests importing the exact repaired core source | Three passes: both committed core tests plus ten real-POSIX-shell quoting round trips (empty, safe, spaces, apostrophe, semicolon, backslash, dollar text, benign substitution text, tab and newline) |
| `cargo fmt --all --check`; `git diff --check 04144d1..HEAD` | Both pass |
| `gh pr view 126 --json headRefOid,statusCheckRollup` | Exact repaired head; CI run 42135722674, all four jobs pass |
| `harness boot`; `harness checks --quick --json` | Same C25/JS tooling limitations: boot NOT-READY; six tooling failures, Copilot-native pass, Rust/smoke skipped |

The initial cargo hint-test selector used `--exact` without its `tests::` prefix and selected zero tests; it is not counted as proof. The fully qualified direct-binary command above actually ran and passed the test.

Only scoped local Rust builds, two jobs, incremental disabled, and the new worktree's own target were used. Disk stayed above the floor (116 GiB at build entry, 114 GiB at final check). No product edits, commits, pushes, dependency links/installations, production-daemon changes or pane interaction.

**Notes:** local full composite proof remains unavailable under C25; exact-head CI is green. The unrelated #122 wall-clock test note from the original review remains unchanged. This is a correctness verdict only; security and performance own their delta verdicts. There is no new steady-state loop, poll, timer or launch site.

Repair receipts are retained in review-session files: `pr126b-templates-http.txt`, `pr126b-independent-replay.txt`, `pr126b-replay-negative-control.txt`, `pr126b-drift-hint-exact.txt`, `pr126b-shell-results.txt`, and `pr126b-harness-checks.json`.

**Harness gift:** the requested command-replay invariant is now committed as a two-repository regression, independently demonstrated red on the original code and green on the repair. Observation GFT-001 records that closure. The existing preflight/C25 mismatch remains a separate harness improvement, not product work owed by this PR.

## Original round: A. Verdict (superseded)

**block** at `04144d1ca39130106202322d3479273c636a388a`.

One P2 correctness finding: a directory selected with `--project` prints commands that drop that project, so following a security/core-role prompt can commit the role into a different project's governance branch. Exact-head CI is green; it does not cover this two-project replay.

Reviewer: pij-subsequent-reindeer. Baseline: `0eebb69ac0e0cf81dd3d98c322d3f0f7ab4308f7`. Reviewed all 38 changed paths. The detached review worktree remains clean at the requested head. No product edits, commits, pushes, dependency links/installations, production-daemon changes or pane interaction.

PR comment: https://github.com/AI-Substrate/pij-fleet/pull/126#issuecomment-6672844433

## B. Summary

The template catalog, version selection, field overrides, writer integration, file-to-brief spelling, embedding guard, upstream-only drift and non-mutating views work in the covered cases. All six templates validate with real ddocs, carry provenance and suitable placeholders, and contain no observed private identities or machine-local paths. Prime explicitly says designation, never assignment (C24).

req-14's statement and done_when correctly replace auto-creation with prompts under C22. Roadmap s03's changed done_when is the corresponding reconciliation, not an unjustified relaxation. PEC wording preserves C18/C26. req-13/14 shipped, req-27 and c-templates in-progress, and s03 complete match this plan's intended state changes, subject to F001 being fixed before merge.

## C. Checks

| Area | Result |
| --- | --- |
| Writer and explicit/latest versions | Pass, including independent distinct-content v1/v2 replay |
| Unknown template/version refusals | Pass; names and available options, no commit |
| All shipped files embedded | Pass; unembedded-file mutation fails, restored control passes |
| Drift and local edits | Pass; v1-to-v2 upstream diff, file bytes and branch unchanged |
| Core/security prompts | Pass for caller-default project; **fail for explicit other project** |
| C18/C22/C24/C26 and document reconciliation | Pass |
| Exact-head CI | Four jobs pass |
| Local composite gate | Dependency/tooling-limited; not claimed green |

## D. Findings

| ID | Priority | Location | Finding |
| --- | --- | --- | --- |
| F001 | P2, blocking | `crates/daemon/src/community/templates.rs:295-310`; related `crates/cli/src/main.rs:2695-2699` | Suggested commands lose the selected project |

## E. F001: preserve the directory's project in its commands

`DirectoryService::view(project, repo)` knows the selected project, but calls `templates::prompts(&made_from)` without it. The generated `pij directory add ...` lines contain no `--project`. The new human drift hint similarly prints `pij directory template-diff <id>` without the view's project.

Reproduced over the real pinned HTTP service and writer with two independent scratch repositories and governance worktrees:

1. Register local project `fleet` and a second project `remote`.
2. From fleet's cwd, request `directory --project remote`. The response correctly identifies remote and suggests `pij directory add security --from template:security --why "recommended role from the security template"`.
3. Replay that exact command from the same cwd. It succeeds with `data.project: "fleet"`; the security role is committed in fleet, not remote.
4. Positive control: an add explicitly carrying `--project remote` commits in remote.

The regression assertion fails with `left: "fleet", right: "remote"`; the local security file exists and the remote security file does not. This is a wrong-repository write, not just misleading text. Outside any registered repo the same unscoped suggestion would refuse instead. A drift hint can also inspect the wrong role or report it missing.

**Required repair:** carry the selected project into generated add commands and the human template-diff hint, using safe argument quoting. Add a two-project HTTP replay regression that checks the selected project and committed path; cover the drift hint's scope too. Keep the existing default-project behavior.

Independent regression source and captured output are retained in the review session's `files/pr126-probe/tests/templates_http/mod.rs` and `files/pr126-project-and-version-probes.txt`. The scratch test links the exact-head library artifacts; it changes no product source.

## F. Acceptance coverage

| AC | Evidence and disposition |
| --- | --- |
| AC1 | Six direct `ddocs validate` runs: each 0 errors/0 warnings; full template/README read; C24 present |
| AC2 | Five committed templates HTTP tests pass, including real-ddocs all-six writer test; independent v1/latest content control passes |
| AC3 | Existing embedding unit passes; exact-source scratch guard rejects an added unembedded file; restored six units pass |
| AC4 | Existing HTTP v1/v2 fixture verifies drift, upstream-only diff, byte-identical local edits, unchanged head and clean scratch worktree |
| AC5 | Existing default-project test passes; independent explicit-project replay fails, F001 |
| AC6 | PRD req-14 statement/done_when and C22 changelog verified; validates/renders without drift |
| AC7 | No new steady-state loop, poll, timer or launch site. Listing/diff/catalog work is in-process; add uses existing writer. Coder's private-daemon receipts corroborate 0 read launches and existing add cost. Independent cost verdict remains performance-owned |
| AC8 | API, skill coverage and procedures read; intended states verified; three project docs and four plan/task/pressure docs validate clean |

## G. Commands and results

All cargo work used the review worktree's own `target/`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=4`, and `-p` scoping. Disk stayed above the 25 GiB floor (124 GiB at entry, 122 GiB at closeout).

| Command | Result |
| --- | --- |
| `cargo test --locked -p pij-daemon --test directory_http -- --test-threads=2` | 18 passed, 1 pre-existing same-second test precondition failed; all five template tests, including real ddocs, passed |
| `cargo test --locked -p pij-daemon --lib -- community:: http::directory http::native_tests a_live_idle_claude_frame --test-threads=2` | 93 passed, including all native cases and the Claude-frame case |
| `cargo test --locked -p pij-core --lib directory` | 1 passed |
| `cargo test --locked -p pij-cli --test skill_verb_coverage` | 4 passed |
| `cargo fmt --all --check` | Pass |
| `ddocs validate templates/roles/<name>/v1/role.dd.json --json` for each of the six names | Six passes, each 0/0 |
| `ddocs validate <path> --json` for changed PRD/blueprint/roadmap, plan, pressure and both task documents | Seven passes, each 0/0 |
| `ddocs build <path> --check --json` for changed PRD/blueprint/roadmap | Three passes, no drift |
| Scratch `directory-probe review_explicit --nocapture --test-threads=1` | Version-content control passes; F001 project-preservation assertion fails |
| Scratch exact-source `templates-probe` | Six baseline passes; adding one unembedded file produces the expected embedding failure; deletion restores six passes |
| `git diff --check 0eebb69...HEAD` | Pass |
| `harness boot` | NOT-READY: missing worktree JS dependencies; no C25 workaround |
| `harness checks --quick --json` | Six tooling/dependency failures, Copilot-native pass, attribution unavailable/warn, Rust/smoke skipped |
| `gh pr checks 126` and head identity query | Run 42132279610: check (22), check (24), rust-lint, rust-test all pass at the requested full SHA |

The five coder-reported load failures are not PR126 product regressions. All native/Claude cases pass in the focused 93-test run. The same-second test fails its own unchanged wall-clock precondition both in the integration run and alone: both holdings are present and the second assign succeeds. The test waits for a changed millisecond timestamp, not a second boundary; the measured standalone operations took about 792 ms but crossed a second. Its assertion and the exercised holding implementation are unchanged by this PR. This is the already-known #122 test-clock problem, not another PR126 blocker.

Receipts retained in review-session files: `pr126-units.txt`, `pr126-cli-coverage.txt`, `pr126-project-and-version-probes.txt`, `pr126-embedding-mutation.txt`, `pr126-embedding-restored.txt`, `pr126-same-second-alone.txt`, and `pr126-harness-checks.json`. The full local composite gate is not claimed passed; exact-head CI supplies JS/toolchain proof under C25.

## H. Handover and harness feedback

Coder owes F001 only; reviewer does not fix product code. Re-review a newly frozen head after the scope-preserving repair and its regression test. Security and performance verdicts remain independently owned.

**magicWand:** command-bearing views should have a deterministic replay contract: executing each suggested command from the original cwd must affect or inspect the view's named project. The failing two-project probe provides that backpressure now, without widening the product change.

**difficulties:** boot recommends dependency linking although governance C25 prohibits it in reviewer worktrees; quick checks also fall through to incompatible ambient JS tools. Existing harness observations DL-001 and SUGG-001 capture the preflight mismatch and the missing two-project replay sensor. A read-only reviewer preflight should report C25/CI-only JS proof rather than recommend a forbidden link. No unrelated harness/product edits made.
