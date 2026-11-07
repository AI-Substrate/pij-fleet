# PR #144 correctness review

## A. Verdict

**block** at `464e06127edccf4356085c6e19075f52e292eef9`.

Two P2 findings: F001 breaks valid doc-test invocations through the canonical shared-build wrapper; F002 treats a failed PA assignment lookup as an empty scope and can broaden the digest to all projects. N001 is a non-blocking diagnostic qualification. N002 is closed by the subsequent coder-supplied real-validator receipt below; the head and blocking verdict are unchanged.

Full plan 209 review against all 16 ACs, from merge-base `3ebbd23787be9239852ac482938b0ca124ea02ec`. The detached checkout is clean and pinned correctly. No product edits, commits, pushes, checkout movement, local builds/tests/probes or live-daemon operations. Security and performance retain their independent verdicts.

## B. Acceptance coverage and plan states

| AC | Disposition against code and evidence |
|---|---|
| 1: flakes | SIGINT handler is installed synchronously before readiness; the EPIPE case retains its payload-record oracle; tmux teardown checks absence; explicit window naming avoids automatic rename; private daemon stop allows its drain. The named cases pass both CI attempts. Reaper removal is explicitly authorized in the plan, not silently counted as a fix |
| 2: test timing | Shared Rust/TS scale has valid/invalid-input tests; registration reads it only under `cfg(test)`, preserving production's one-second constant. Selected real-child test budgets and driver shutdown budgets use it |
| 3: skill check | Added to CI, `harness checks` and `just self-check`; changed markers retain checks or give retirement reasons. Both Node jobs' check-stage runs pass; no reviewer mutation/local run |
| 4: delivery card | The recorder comes from an address row at the selected newest time. Both reads share a transaction; the two-recorder regression passes |
| 5: PA scope | Normal own/parent/designated-prime selection, SQL project restriction, bodiless projection and delivering-row visibility are present and tested. **F002 leaves the checked claim incomplete on lookup failure** |
| 6: late admission | Owned admission task is retained past the bound; the held action runs before its join; the final event classifies late delivery/refusal. Both outcomes and the bound regression pass |
| 7: cost | Source cost shape reviewed below. Live AFTER remains deferred by the packet; backpressure row bp-0007 correctly remains unchecked, despite the broader AC's checked state |
| 8: docs/states | Correctly unchecked; plan remains draft and phase unchecked. No project/roadmap state changes appear in the diff. PR body is `Delivers: none`, and its dedicated check passes. ddocs CI is explicitly UNAVAILABLE; local coder documentation evidence is not relabeled as reviewer proof |
| 9: ledger follow-ups | HUP shares whole-process-group cleanup; memory prompt asks for one note; shim stdin test checks bytes and argv with a recorder, paired with the native body-inlining unit test. Forwarding fixes inherited from plan 213 are attributed separately |
| 10: memory UX | Explicit slug validation/path use and 40-character word-boundary derivation are tested; the success renderer is wired only for successful non-JSON memory adds. Local date is inherited from #138 and its regression passes |
| 11: add help/refusals | Required flags and per-add commit semantics match the schema. Formatter tests cover missing-field flags. **N002 closed:** coder's real-validator receipt supplies three gaps; the regression is the all-flags next step, not newly introduced issue aggregation |
| 12: Delivers gate | New binary uses the same parser as `pij delivered`; executable tests cover valid addresses, none, prose, absent line and the template placeholder. Dedicated workflow succeeds on this head |
| 13: lineage batch | Prime gate, time validation, under-lock role/overlap checks, ordered receipts and one authoritative commit are implemented. Integration test proves refusal/no commit and success/one commit. **N001 qualifies the checked "every bad line" claim** |
| 14: delivered output | Existing native renderer/forwarding prints edit commands; new regression exercises two commands |
| 15: foreign edits | Refusal retains dirty paths and now directs the seat to the prime; writer test checks the next step and unchanged files |
| 16: split scheduling | Ordinary cargo-test split and runner arguments are covered by fake-cargo CI. **F001 means the checked claim cannot be accepted for all valid cargo-test forms** |

No whole-plan completion is approved. AC5/AC16 need repair; AC11/F37's evidence qualification is closed by the receipt below; AC13's diagnostic wording must not be presented as exhaustively proved.

## C. F001: doc-only tests are rewritten to invalid Cargo arguments

**P2, `harness/scripts/shared-build.sh:343-374`.**

Every `test` command lacking an explicit `--no-run` enters `build_only`, which preserves its Cargo arguments and appends `--no-run`. Therefore:

```text
just shared-build -- test -p pij-core --doc
  -> cargo test -p pij-core --doc --no-run
  -> failure before the requested run
```

The repository pins Rust 1.98.0. Its Cargo submodule is `797e8a9bca276c1c9f9f738d2a20f484fa4eea9d`; the [pinned command implementation, lines 86-96](https://github.com/rust-lang/cargo/blob/797e8a9bca276c1c9f9f738d2a20f484fa4eea9d/src/bin/cargo/commands/test.rs#L86-L96), rejects this pair with `can't skip running doc tests with --no-run`. This is source proof against the exact toolchain, not a local execution claim.

This repository itself uses `cargo test --locked --workspace --doc` in `.github/workflows/ci.yml:135`. CI invokes it directly, bypassing shared-build. The wrapper's fake cargo accepts arbitrary flags, so both green runs miss this compatibility regression.

**Repair:** preserve valid doc-only commands with an explicit supported scheduling policy instead of blindly adding `--no-run`. Add CI coverage through the actual wrapper for this mode, retaining ordinary compile/run and runner-argument tests. Also qualify the blanket "second call compiles nothing" statement: ordinary Cargo doctests can compile on the fly during the run phase, as the [Cargo documentation](https://doc.rust-lang.org/cargo/commands/cargo-test.html#documentation-tests) explains.

## D. F002: an unavailable assignment scope can become all projects

**P2, `crates/daemon/src/pa_watchdog.rs:1068-1116`.**

`held_projects` discards `open_assignments` errors with `unwrap_or_default`. `queued_requests` cannot distinguish that failure from a successful empty result and applies the broader parent/designated-prime fallbacks.

Concrete dataflow: a PA holds a role only in project A; its parent is the designated prime and has no role assignments. If the PA's lookup errors, the parent's successful empty lookup and prime lookup select `digest_scope([], [], true) == None`, and `unsettled_role_requests_in(None)` reads requests from every project, including B. A single failed scope read can thus undo the successful-path restriction without an unavailable/error indication.

`open_assignments` is fallible SQL plus row decoding, not an infallible in-memory list (`crates/store/src/community.rs:277-287`). The prime designation is a separate read. The store's error type must not be promoted to proof of no assignments.

**Repair:** preserve the error, report the queue portion unavailable using the existing diagnostic pattern, and do not broaden scope on failed reads. Add a deterministic service-level CI case in which the PA scope lookup fails while parent/prime/request reads succeed; assert no out-of-scope requests are returned. Existing tests cover the pure successful-result chooser and the store selector, not this wiring/error path.

This is a correctness/error-handling finding. The assigned security reviewer was separately notified and owns the security assessment.

## E. Non-blocking notes

**N001, P3: lineage diagnostics remain phase-local.** At `http/directory.rs:910-961`, one time/syntax error returns before missing-role/overlap validation. Service errors name roles/spans but do not retain file line numbers. A batch mixing a reversed span, a missing role and an overlap therefore does not initially name every bad line. Current tests separate the reversed case from the other cases. Atomicity is intact. Narrow the promise or extend the validation/coverage; do not claim exhaustive diagnostics from these tests.

**N002, closed on the unchanged head.** The original proof concern was that `a_role_refusal_lists_every_gap_at_once` supplies all three issues itself and the previous formatter already iterated them. Warbler supplied the missing producer receipt in message `b20543a80c49a202-000000000000005b`: a scratch copy of the coder role fixture, using this worktree's schemas and the daemon-resolved ddocs, passed unedited with zero issues; removing kind, scope and standing_packet returned exit 1/E402 with three issues at `$.sections[meta].value.kind`, `$.sections[scope]` and `$.sections[standing_packet]`.

The receipt confirms ddocs already aggregates the issues. The F37 fix is replacing its misleading first-gap-only `next_action` with one next step naming every required flag. `a_role_refusal_names_the_flag_for_each_gap_not_a_hand_edit` checks that all-flags ending and absence of the old hand-edit instruction, and would fail on the baseline. The issue-lines-only test is retention coverage, not evidence of a newly fixed aggregator. AC11/F37 is accepted on that basis. This is coder-supplied execution evidence, not a reviewer local run or CI validation; no code change or push was needed.

## F. CI evidence

Run **42222861644**, attempts **1 and 2**, both name the exact head and pass check (22), check (24), rust-lint and rust-test. Each Rust summary reports **2,020 passed, five skipped**. Downloaded attempt-2 JUnit records 2,020 executed cases with zero failures/errors.

Both Node 22 logs report **2,202 passed, six skipped**, including **16 shared-build tests** and the new time-scale test; `pij-skill-check` is all green. Both ddocs steps explicitly report UNAVAILABLE. Dedicated **delivers run 42222861585** succeeds at the exact head and prints `Delivers: none`; a cancelled older body-check run is superseded, not a failing current gate.

Named receipts include `a_later_recording_by_another_seat_names_that_seat_with_its_time`, `a_late_admission_is_recorded_after_the_action_never_withdrawn`, `a_lineage_batch_is_one_commit_all_or_nothing`, both role-refusal/help tests, slug validation, both human renderers, scoped request recovery and the executable Delivers test. Their narrower proof boundaries are stated above.

## G. Commands, cost and limits

Read-only git HEAD/status, merge-base and full source/test/docs diffs; source/schema reads; `pij directory routes`; PR metadata/body; both CI attempts' metadata/logs; dedicated delivers log; downloaded JUnit; immutable public Rust/Cargo source inspection. No local harness gate, build, test suite, mutation or runtime probe was executed under the CI-only instruction.

Artifacts: `<reviewer-session-files>/pr144-464e061-code.diff`, `pr144-464e061-docs.diff`, `pr144-464e061-ci/junit.xml` and the preliminary findings file.

Cost shape: up to two assignment-scope reads and one prime read per digest, one queued-index seek per scoped project **plus one global delivering-index read with project filtering**; bodies are not selected. No new digest timer or per-seat launch. Late admission adds one owned Tokio task per turn, no process. Lineage batches amortize one writer commit. Shared-build adds a Cargo/nice invocation per ordinary test command; HUP adds no steady-state work. Existing spawn-site guard passes in CI; independent performance owns final cost/QoS proof.

After deploy, performance/the merger should measure per-site spawn-counter deltas over a known interval on one daemon incarnation, the whole-machine `just spawn-rate --seconds 20` receipt, daemon CPU/RSS and the affected PA-digest workload. The supplied 0.70/s BEFORE receipt is coder evidence; AFTER remains owed as explicitly permitted by the packet.

## H. Handover and harness feedback

Keep the head frozen until a repair round is authorized; no reviewer fixes are applied. Return a new frozen head with focused CI regressions for F001/F002, preserving the existing green cases. Carry N001 as an explicit qualification rather than marking all 16 ACs proved; N002 needs no repair.

**magicWand:** test the actual command/service boundary, not only fake-cargo argv or a pure scope chooser. Those are exactly the two green-test gaps that hid these findings.

**difficulties:** aggregate green CI and named regression tests do not establish the external validator's behavior or every Cargo mode. This review records each proof ceiling and uses the pinned producer contract where local execution is forbidden.
