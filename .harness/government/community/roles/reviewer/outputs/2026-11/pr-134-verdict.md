# PR #134 correctness review

## A. Verdict

**block**, exact head `d85c0f3adab39895dd6776422aaa5b3ad4048d8c`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/134#issuecomment-6674309986

F001 is a source-proven P2: the stated at-most-one-ps-per-registration budget is not implemented. The implementation guarantees one ps per host-named ancestor instead. N001 is a nonblocking P3 qualification of the new source guard. No UTC-offset or registration-identity regression was found.

Review is source plus exact-head CI only under the fleet backoff. No local builds, tests, process probes, mutation runs, private-daemon scripts, boot or checks were run. The counterexamples below are source traces, not claimed runtime reproductions.

## B. Scope

Plan 207, issues #127/#128, C27; the functional diff, its callers, the unchanged native/paneless ancestry and identity checks, manifests, tests, plan/backpressure and committed evidence. The new detached review worktree remains at the requested SHA with no tracked edits. No commit, push, deployment, daemon restart or new seat.

## C. Checks that pass

- **UTC offset:** `crates/harnesses/src/proc.rs:343-358` uses the existing `time` local-offset feature, at the current instant, and returns whole minutes with explicit adapter failure rather than inventing an offset. macOS refreshes TZ before conversion, as the accepted ADR 0001 path does. The binder, FYI claim/read, carrier flush, adopt, revive and double-seat callers retain the same API. The per-pending-Claude-seat five-second loop no longer launches date.
- **DST and parity:** CI JUnit records the multithreaded Melbourne +11/+10 fixture, parity with `/bin/date +%z`, and the PATH-decoy test passing. The no-launch assertion runs in the isolated DST child, not across unrelated tests. ADR 0001's existing Linux system-timezone-change/restart limitation remains applicable; ordinary DST transitions are not that limitation.
- **Parent/name:** macOS uses the existing `ProcBSDShortInfo` wrapper, verifies the returned pid, and returns its parent and accounting name. The documented 15-character truncation does not truncate any of the four accepted host names. Linux's `/proc` implementation is unchanged.
- **Registration identity:** both walks still cap at 32 processes, reject cycles and newer-parent incarnations, and sandwich observation with start-time rechecks. Native registration still verifies the actual claimed Copilot host and exact pane ancestry. Paneless registration still checks the nearest recognized external harness and its native session. Non-host names do not satisfy these matchers.
- **Safety/dependencies:** `unsafe_code = "forbid"` remains unchanged; no new unsafe block, dependency, manifest or lockfile change. No new timer or process-launch site. The retained ps launch remains absolute, bounded and counted.
- **Retired date tripwire:** `retired_launches_stay_retired` rejects the old operation label across daemon sources and the original `/bin/date` construction in proc.rs, with a positive scan-coverage assertion. This is a literal-source tripwire, not a proof against every possible alternative spelling or helper.

## D. Findings

| Id | Severity | Location | Finding |
|---|---|---|---|
| F001 | P2, blocking | `crates/daemon/src/registration.rs:1644-1666`; loops at `:223` and `:437`; count test at `:4652-4686` | Each eligible ancestor launches ps independently; there is no one-per-registration bound. |
| N001 | P3, nonblocking | `crates/testkit/tests/spawn_sites.rs:340-377` | The new guard does not establish that the production wrapper actually delegates through the host-name gate. |

## E. Details and concrete fixes

### F001: the cost scales with host-named ancestors, not just registrations

`read_native_process_from` calls `with_argv(pid)` independently whenever the current name is `copilot`, `claude`, `codex` or `node`. Both registration walks call it again for the next ancestor. No registration-scoped budget or batched read is present. The source comment at :1646 accurately says “per host-named ancestor”; AC2/AC3/AC5, the PR body and operating docs claim the stronger “at most one per registration”.

A native chain accepted by the existing verifier demonstrates the difference:

| Process | Parent | Observation |
|---|---|---|
| actual `copilot` host | node launcher | First ps; actual-host check passes. |
| `node launcher.js` | pane's zsh | Second ps solely because the accounting name is node. Native ancestry does not require this parent to be another recognized harness. |
| pane's zsh | earlier parent | Parent/name in-process; exact pane identity reached; existing sandwich checks can all pass. |

Likewise, a paneless `pij -> node helper.js -> node <recognized Claude entrypoint>` chain reads argv for both nodes before accepting the same Claude host/session the old code accepted. Node launchers/helpers are not constrained away by either registration API. Up to the unchanged 32-ancestor cap can be host-named; the host-name check does not imply a count of one.

The count test is real: it runs `register_paneless_with_observer`, traverses the actual gate helper, increments the injected argv reader and checks the resulting host. It is not a constant-return fake. However, every fixture is specifically `pij -> sh -> one host`, so the expected count of one is guaranteed by fixture shape. The macOS real-ancestry test checks count equals the number of host-named ancestors, not count <= 1, and is not a native registration budget test.

**Fix:** retain the host-name restriction and incarnation checks, but make the read registration-scoped. Native registration can avoid argv reads for ancestors after the claimed host has been verified; paneless registration can collect bounded host-named candidates and use one bounded, exact-PID batched ps argv read before applying the existing nearest-host/session rules. Add native and paneless CI cases containing more than one node/host-named ancestor and assert both the preserved registration result and the promised budget. Do not silently reject a previously supported chain just to satisfy the counter.

Alternatively, obtain an explicit prime decision accepting cost proportional to host-named candidates and amend the hard budget and proof claims consistently. C27's recorded text says “about one”, but this task's packet and the current plan explicitly require “at most one”; the reviewer cannot silently replace that contract.

**Cost classification:** the host-name gate is a one-directional brake: removing it reads argv for the same set or more ancestors. It is useful and correctly implemented, but it does not cap the number of ancestors that satisfy it. This matters because #128 documents clients that can retry registration on timers.

### N001: the standalone source guard has a straightforward false-negative shape

Replace only the body of `read_native_process` with `read_argv_with_ps(pid)`, leaving `read_native_process_from` in the file. The guard's observed facts stay identical: the operation is recorded in `read_argv_with_ps`; its one mention is still in `read_native_process`; the unused helper still contains `HOST_NAMES.contains` within the first twenty lines. Thus this guard does not reject that direct-return restoration of per-ancestor ps.

This is a source-level counterexample to the guard, not a claim that the entire workspace gate would pass: another lint or test could reject that particular mutant. The coder's reported mutation run is not independently repeated under the backoff.

**Fix:** also assert that the production wrapper delegates through `read_native_process_from`, and add the exact direct-return replacement as a negative control for the guard in CI. Keep the behavioral non-host/no-argv test as the stronger proof of the gate helper. No parser dependency or broad abstraction is needed.

## F. Acceptance and proof ceiling

| AC | Disposition |
|---|---|
| AC1 | Source and Linux CI support the no-date helper, current-offset parity and DST fixture. Caller signature/behavior paths unchanged. |
| AC2 | Parent/name lookup and identity rules are sound by source; hard one-per-registration budget blocked by F001. |
| AC3 | Date tripwire and six single-candidate paneless count variants pass CI. F001 limits the budget proof; N001 qualifies the standalone gate tripwire. |
| AC4 | Correctly unchecked: live-fleet before/after receipts remain performance-owned. No live measurements performed or inferred here. |
| AC5 | Date loop cost becomes zero launches. Registration becomes one ps per host-named candidate, not the claimed hard one per registration. F001 blocks that claim. |
| AC6 | No new verbs or project-state changes are needed for this issue fix. Documentation names the new mechanics, but the hard budget wording must follow F001's resolution. |

CI is Ubuntu only. The new `parent_and_name_match_ps_without_a_launch` and `a_real_ancestry_matches_ps_and_launches_only_for_host_names` tests are macOS-gated and therefore are not evidence produced by this CI run. The committed historical coder receipts cover macOS, but they are not independent reviewer execution or a live-fleet after-deploy measurement. No extra local run is requested under the backoff.

## G. CI evidence and commands

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42143552183

- `gh run view 42143552183 --repo AI-Substrate/pij-fleet --json headSha,status,conclusion,jobs`: exact reviewed SHA; completed success for Node 22, Node 24, rust-lint and rust-test.
- Rust job `127649456621` log: nextest summary **1,896 passed, five skipped**. Downloaded `junit-linux/junit.xml` records 1,896 test cases, zero failures and zero errors. These are two views of the same CI origin, not independent runtime proofs.
- JUnit confirms passes for the two UTC tests, the UTC PATH-decoy test, `every_host_name_a_matcher_accepts_reads_argv`, `paneless_registration_reads_argv_only_for_the_host_named_ancestor`, both 32-ancestor controls, the first-light spawn-counter control, and both new spawn-site tripwires.
- CI's executed gate commands include `cargo metadata --locked --format-version 1`, `cargo fmt --all --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace --doc`, and `cargo nextest run --locked --workspace --profile ci`: successful in their jobs. These were not run locally by this reviewer.
- `gh pr view 134 --repo AI-Substrate/pij-fleet --json headRefOid,mergeStateStatus`: exact requested SHA remains current; CLEAN.
- Read-only git inspection confirms no worktree changes. Manifest/lockfile diff is empty. Source-only counterexamples and independent mutation execution are not conflated.

## H. Handover and harness gift

Close F001 by implementing the promised bound without changing registration acceptance, or by obtaining an explicit revised budget. N001 is a focused guard-hardening note. Re-review the frozen repair SHA using CI only while the backoff holds; no new local build window or seat is requested. #131 remains a separate pending repair, with its withdrawn state-printing concern still withdrawn.

**magicWand:** make the registration cost fixture vary the number of host-named ancestors, not just the harness and native-versus-node form. A two-node chain would have made the claimed bound fail in CI before review.

**difficulties:** the first `gh run view --log` retrieval yielded no named-test lines. The raw job-log API supplied a positive content control and run summary; the published JUnit artifact supplied the actual named-test receipts. CI-only macOS work needs that proof ceiling stated rather than treating green Ubuntu CI as execution of platform-gated tests. No local workaround or probe was used.
