reviewer: block (head b28d815be55ee4698d308eea1b67d91c3e4d0fd2)

Two P2 correctness findings. Reviewed the frozen detached worktree, not a moving PR head. No product edits, commits, pushes, deployment, production-daemon testing or test writes to live governance were made. Security and measured-cost verdicts remain with their assigned reviewers.

Supersedes the earlier verdicts at d5fd8cf and 0c6ea88. Per the PM's final-head request, inspected the complete `git diff d5fd8cf b28d815`: only four documentation files change. They record the lifecycle-test flake, renumber difficulty rows, correct bp-0005's estimate to the measured 9/add and 7 + 2N/batch cost, and add the coder's shared-tooling receipts. The cost note now agrees with the existing transcript; the shared-tooling results are attributed to the coder, not claimed as this reviewer's local gate. The delta has no new correctness finding. All production code, schemas and tests are identical, so F001/F002 and the existing runtime receipts carry forward unchanged. No Rust rebuild was needed; the checkout stays at d5fd8cf.

## Findings

### F001 [P2] Reject a nonexistent standing-packet brief before committing

Location: `crates/daemon/src/community/ddocs.rs:110-111`, with the packet check at `crates/daemon/src/community/role.rs:182-205`.

The role contract requires `source: brief` to reference an existing file. `check_packet` only checks that the string is nonempty, and `Ddocs::run` treats every zero exit code as validation success. Real ddocs reports a missing file link as a warning with exit 0. Consequently a role with `brief: "briefs/definitely-absent.md"` is committed successfully, including in an otherwise valid batch. The stored role has no usable standing packet.

Independent reproduction: a real-ddocs scratch writer accepted a two-role batch containing one valid inline packet and one missing brief, returning `Ok(Written { commit: "56a7be9df470926fb07d7715855d490ad471259e", ... })`. The regression expecting refusal and no commit fails. The positive control, with a committed `briefs/qa.md` in the same governance-root layout, passes.

Fix: check that the brief resolves to an existing readable file in the governance worktree, or promote this specific missing-brief diagnostic to a named refusal, before committing any batch path. Include the role and brief path in the refusal. Do not blanket-reject unrelated advisory ddocs warnings. Add both existing/missing-brief controls and assert that the missing-brief batch leaves HEAD and the index unchanged.

### F002 [P2] Make a PATH-discovered ddocs path absolute before changing cwd

Location: `crates/daemon/src/community/ddocs.rs:72-74` and `:96-98`.

The PATH fallback accepts any resolver result containing `/` as `Found`, including relative entries such as `tools/ddocs`, `./ddocs` and an empty PATH component's `./ddocs`. The child then changes cwd to the governance worktree. The accepted executable path is now relative to another directory, so an installed ddocs fails as `E-RS-DDOCS-MISSING` instead of running the program resolved at daemon startup. This contradicts AC5/C10's resolved-once, absolute-path contract.

Independent reproduction: the same executable launched successfully with an absolute `PIJ_DDOCS` path after cwd changed. Resolving it through a relative PATH entry returned `Found("pr114-relative-ddocs-54407/ddocs")`; validation from the scratch governance directory returned `E-RS-DDOCS-MISSING`, OS error 2.

Fix: resolve the winning PATH candidate to an absolute executable path at discovery time and retain that path. Add execution-level controls for relative and empty PATH components, with a different governance cwd, alongside the existing absolute-path and explicit-override tests.

## What passed

- AC4/C8: real scratch repositories preserve unstaged, staged and untracked edits; commit only pij's paths; preserve pij's files in the next ordinary commit; refuse named foreign edits; serialize concurrent writes; retry and then refuse a held index lock without removing it. An added commit-hook failure control also preserves the foreign index bytes and worktree contents.
- Safety mutation: disabling the foreign-edit guard makes the committed foreign-edit regression fail. This guard is a **one-directional safety interlock (a brake), not a policy**: removing it permits more writes; it does not select a different outcome policy.
- AC6: existing eight-role HTTP import and invalid-document all-or-nothing tests pass. Inline/brief source-field pairing tests pass. F001 is the additional missing-file case the fake validator does not cover.
- AC7: row-for-row pointer rebuild from real branch history passes, as does store upsert/project isolation. Migration 0031 adds only `community_pointers` and advances schema version 30 to 31.
- AC8, static scope only: both new launch sites use the program-launch helper and record counters; no timer/poll/per-seat loop was added. Removing both records makes `spawn_sites` fail naming `community/ddocs.rs::run` and `community/git.rs::git`; restoring them makes all four checks pass. The supplied private-daemon receipts say 0 launches/60 s at rest, 9/add and 23/eight-role batch. This is not a separate live-cost verdict.
- AC9/AC11: PRD, blueprint and roadmap validate with zero errors/warnings. req-03 matches C8; the selected shipped/in-progress states, s02 complete, and req-26's partial naming status match the plan. Required role fields, `default_limit`, output kinds and memory fields are present.
- AC10: ping returns the serving process's pid and a ddocs field. Doctor reads the daemon-resolved ddocs path. Absolute override precedence and named missing/invalid-tool refusals pass; F002 is the uncovered relative-PATH case.

## Validation receipts

All cargo invocations used `CARGO_TARGET_DIR=<the coder worktree target>`; no review-worktree target was created. Available disk stayed above the 25 GiB floor (31 GiB initially, 27 GiB at closeout).

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --lib community -- --nocapture` | PASS: 24 tests; real ddocs ran, not skipped |
| `cargo test --locked -p pij-daemon --test directory_http -- --nocapture` | PASS: 3 tests |
| `cargo test --locked -p pij-store --test store_integration community_pointers -- --nocapture` | PASS: 1 test |
| `cargo test --locked -p pij-testkit --test spawn_sites -- --nocapture` | PASS: 4 tests |
| `cargo test --locked -p pij-daemon --lib ping_reports_the_daemon_pid -- --nocapture` | PASS: 1 test |
| Scratch extraction: `cargo test --locked --manifest-path <scratch>/Cargo.toml -p pij-daemon --lib community:: -- --nocapture` | 26 PASS, 2 FAIL: the independent F001/F002 regressions above; production source restored after mutations |
| Scratch foreign-edit-guard mutation: `cargo test --locked --manifest-path <scratch>/Cargo.toml -p pij-daemon --lib a_path_carrying_a_foreign_edit_refuses_and_writes_nothing -- --nocapture` | EXPECTED FAIL with guard disabled; passes restored |
| Scratch counter mutations: `cargo test --locked --manifest-path <scratch>/Cargo.toml -p pij-testkit --test spawn_sites every_daemon_launch_site_records_itself -- --nocapture` | EXPECTED FAIL naming both removed counters; all 4 tests pass restored |
| `ddocs --json validate <fixture> --depth 0` on all 9 new fixtures | 7 expected PASS; 2 expected E402 refusals for missing scope/packet |
| `ddocs --json validate <project-document> --depth 0` on PRD, blueprint and roadmap 001 | PASS: all 3, zero errors/warnings |
| `git diff --check origin/main...HEAD` | PASS |
| `git diff --check d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed 0c6ea8886e6f4dec9cc09d61cdcd654157be28ae` | PASS; complete delta read, only 2 Markdown files |
| `git diff --check d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed b28d815be55ee4698d308eea1b67d91c3e4d0fd2` | PASS; complete final delta read, only 4 documentation files |
| Exact-b28d815 snapshot: `ddocs --json validate docs/plans/190-community-writer/assets/backpressure.dd.json --depth 0` | PASS: zero errors/warnings |
| Exact-b28d815 snapshot: `ddocs --json build docs/plans/190-community-writer/assets/backpressure.dd.json --check` | PASS: no generated Markdown drift |
| `harness boot --json` | Advisory NOT-READY: no linked-worktree node_modules |
| `harness checks --quick --json` | FAIL: 5 JS/tooling stages; Copilot-native PASS; attribution WARN; Rust and smoke explicitly skipped |

Local TS/full lint/smoke/full-workspace checks are not claimed green. The packet forbids dependency linking/install and scopes Rust builds. GitHub CI is green for all four jobs on both the tested d5fd8cf ([run 41768673271](https://github.com/AI-Substrate/pij-fleet/actions/runs/41768673271)) and this verdict's exact b28d815 ([run 41769699230](https://github.com/AI-Substrate/pij-fleet/actions/runs/41769699230)). Those green checks do not cover the two reproduced cases.

At this follow-up, GitHub reports the later head `8eb245b868dd6ecd4aefaaaabd7f0de89a378dab`. This verdict is pinned to the explicitly requested `b28d815be55ee4698d308eea1b67d91c3e4d0fd2`, not to that newer head. The review worktree was never moved.

## Handoff and harness feedback

Fix F001 and F002, retain their deterministic regressions, and request review of a new frozen head. No schema expansion or unrelated warning policy is needed.

Local reproducible evidence: session `e1e17389-7c15-4043-a345-a4d5df30b473`, `files/pr114-regression-tests.patch` (four reviewer tests, applies to the frozen SHA) and `files/pr114-probe-results.txt` (actual execution output). The patch is review evidence, not an applied product change.

**magicWand:** keep a real-ddocs missing-link batch regression beside the existing fake-validator tests. This is the highest-value encoding: the schema can truthfully warn while the product must refuse an unusable standing packet.

**difficulties:** local composite proof is unavailable in the intentionally dependency-free review checkout; PR documentation heads moved while the assigned review SHA stayed frozen. Captured as DL-001/MW-001 in the review worktree's ignored harness buffer. No shared harness state was rewritten.
