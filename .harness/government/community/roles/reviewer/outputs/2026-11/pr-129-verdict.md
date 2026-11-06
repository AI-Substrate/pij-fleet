reviewer: pass-with-notes (head 314da13a19d8d233e8c7b6d48d3de0991635b47e)

## A. Current verdict

F003 closed. F001/F002 remain closed. No remaining correctness changes requested for this review scope. Security's S1/S2 and recovery-admission verdict, and performance's boot/digest cost verdict, remain separately owned.

Reviewed the F003 repair and relevant merge wiring in the clean, frozen review-129-314da13 worktree. CI-first: no local builds, test runs, runtime probes or production changes.

## B. F003 closure

Boot now calls recover_requests after winning the listener bind and before starting delivery/watchdog workers or serving requests. A losing boot cannot reconcile the running daemon's claims.

recover_delivering_role_requests settles the prior daemon's delivering rows in one write transaction. Matching local-origin delivery ledger, delivery:<holder> job or FYI evidence retains the original holder and marks the request delivered. A claim with no admitted delivery returns to queued with no holder. This avoids both the invisible orphan and the unsafe blanket requeue that could reopen F001/F002 after an admitted-but-unmarked send.

The PA path now reads unsettled_role_requests: queued and delivering rows through their respective partial indexes, without reading message bodies. The digest shows delivering claims separately with holder and waiting age, so a failed recovery remains observable. Recovery failures are explicitly logged.

## C. Exact-head CI

Run [42174918545](https://github.com/AI-Substrate/pij-fleet/actions/runs/42174918545), head_sha 314da13a19d8d233e8c7b6d48d3de0991635b47e, passes check (22), check (24), rust-lint and rust-test.

Downloaded junit-linux artifact 13482338801 and inspected the testcase records, not only the job conclusion:

| Test | Result |
|---|---|
| directory_http::a_claim_left_by_a_stopped_daemon_drains_once_after_restart | Passed |
| store_integration::role_requests_left_delivering_are_recovered_at_boot | Passed |
| pa_digest::digest_lists_queued_role_requests_only_when_there_are_some | Passed |
| directory_http::review_single_holder_retry_is_deduplicated_control | Passed |
| directory_http::review_replay_after_reassignment_must_not_reach_a_second_holder | Passed |
| directory_http::review_overlapping_assign_drains_must_deliver_an_unscoped_request_once | Passed |
| directory_http::security_a_released_holder_never_drains_and_a_cold_drain_is_audited | Passed |
| Existing one-holder, several-holder/instance and no-holder/ordered-drain HTTP cases | Passed |
| store_integration::role_requests_queue_per_role_oldest_first_and_deliver_once | Passed |

The nextest log reports 1968 run, 1968 passed, 5 skipped. The named cases above executed; they were not skipped.

## D. What the restart proof establishes

The new daemon test seeds two committed claims against a persistent database: one not yet handed off, and one already admitted as a held FYI. It calls the real boot, observes queued/no-holder versus delivered, then rotates the holder and proves that only the unsent request drains, exactly once, to the new holder.

The store test separately verifies matching ledger/FYI settlement, a wrong-recipient ledger negative, preservation of already-queued/delivered rows, recovery idempotence, and body-free visibility of both unsettled states. The digest fixture checks that a delivering claim names its holder and is not mislabeled as queued.

## E. Merge and prior findings

The role-send HTTP implementation is unchanged from a560297; the only delivery/role_send.rs addition is recovery. The holder-checked claim and cold-drain path are retained. The merged http/mod.rs still invokes role_send::route, and the test rig retains its template catalog, real drain binding and new main-owned memory-turn fields.

F001/F002 stand from a560297 and their regressions pass again on this head. The unrelated main changes were not re-reviewed as new s05 work. SCHEMA_VERSION remains 34; migration 0034 adds the delivering partial index.

## F. Checks and proof limits

git diff --check a560297..314da13 passed. Worktree HEAD and GitHub's head/run attribution agree.

No local cargo, just, harness checks or test invocation was needed or run. The new restart fixture exercises boot and durable state reconstruction, not an actual process kill. The new store fixture directly covers ledger/FYI admission; the delivery-job evidence branch is verified by source inspection rather than a dedicated new fixture. The reported red-without-boot-call mutation was not rerun by this reviewer.

## G. Disposition

F001, F002 and F003 are closed at this exact head. No new correctness blocker. Notes above qualify proof, not required changes. Do not carry this verdict to a moved head without reviewing its delta.

## H. Harness improvement

The three review rounds now leave deterministic replay, competing-drain and interrupted-claim/boot regressions in CI with downloadable JUnit receipts. Recovery and PA visibility are proved through the real integration points instead of relying on a successful store UPDATE alone.

<details>
<summary>Historical block at a560297ec107c4f7168cc4400a4224d689f59b05</summary>

reviewer: block (head a560297ec107c4f7168cc4400a4224d689f59b05)

## A. Current verdict

F001 and the original F002 duplicate-dispatch counterexamples are fixed. One new P2 blocker, F003, is introduced by their durable delivering state: interrupted claims have no restart recovery and disappear from the role queue and PA digest.

Reviewed da23eb7..a560297 in the clean, frozen review-129-a560297 worktree. CI-first: no local builds, test runs, runtime probes or production changes.

## B. Repair review

- F001 closed: request lookup precedes holder resolution; completed/queued retries return stored metadata without sending. Immediate dispatch records its pinned holder before sending. The rotation replay, unchanged-holder control and conflicting-body refusal are covered by the adopted regressions.
- Original F002 closed: queued-to-delivering CAS precedes dispatch, and a competing drain skips a lost claim. The adopted competing-instance regression checks both the total inbox count and that the durable holder is the winner.
- Merge resolution remains correct: rig_with/rig_full and the template catalog are preserved; rig_inner(None) constructs and binds the real drain. The unrelated #122 submodule remains enabled.
- The new bind/without_hook wiring avoids the direct directory-to-hook-to-directory cycle and retains the template catalog. Security owns S1/S2 and the intentional cold-holder policy; performance owns digest scaling and claim cost.

## C. Evidence

Exact-head CI run [42151561097](https://github.com/AI-Substrate/pij-fleet/actions/runs/42151561097) passes check (22), check (24), rust-lint and rust-test. Its head_sha is a560297ec107c4f7168cc4400a4224d689f59b05.

Downloaded junit-linux artifact 13472488784 and inspected its actual testcase records. These all pass:

```text
pij-daemon::directory_http
  review_single_holder_retry_is_deduplicated_control
  review_replay_after_reassignment_must_not_reach_a_second_holder
  review_overlapping_assign_drains_must_deliver_an_unscoped_request_once
  a_role_send_reaches_its_one_holder_and_records_the_request
  several_holders_refuse_by_name_and_an_instance_reaches_only_its_holder
  no_holder_queues_wakes_the_pa_and_an_assign_drains_in_order
  security_a_released_holder_never_drains_and_a_cold_drain_is_audited
  review::review_reassign_within_one_second_keeps_both_holdings
pij-store::store_integration
  role_requests_queue_per_role_oldest_first_and_deliver_once
```

The nextest log reports 1900 run, 1900 passed, 5 skipped. These are CI receipts, not newly executed local tests. The project docs are unchanged from the previously validated da23eb7 state.

## D. Open finding

| ID | Severity | Location | Concrete fix |
|---|---|---|---|
| F003 | P2 | crates/daemon/src/delivery/role_send.rs:727-748; crates/store/src/role_requests.rs:179-207,225-238 | Make a committed delivery claim recoverable after daemon interruption, using a durable handoff or startup reconciliation with the original holder and delivery queue/ledger |

## E. F003: a stopped drain permanently hides its claimed request

Static execution trace:

1. A previously acknowledged role request is queued.
2. An assign starts its drain. claim_role_request commits state=delivering and holder=A in its own transaction before deliver_drained is called.
3. Stop the daemon after that commit but before the delivery is admitted. No delivery job exists yet, and the task cannot execute its error-path release.
4. Restart and assign a holder. queued_role_requests selects only state='queued', so the request is skipped forever. all_queued_role_requests uses the same predicate, so the PA also loses visibility of the still-unprocessed request.

The only release path runs in the original in-memory drain after an observed Err. RoleQueueDrain::bind and build_services do not reconcile delivering rows. Source references to role_requests show no other startup recovery writer. The requester already received a successful queued receipt, so a new HTTP retry is not guaranteed and is not queue recovery.

This is a new liveness regression caused by persisting the F002 claim outside the durable delivery handoff. Recover it without reopening F001/F002: a crash can also occur after delivery was admitted but before it was marked delivered, so blindly resetting every delivering row to queued can dispatch the same request to a rotated holder. Reconcile the original holder/message id against durable delivery state, or commit the handoff atomically.

Proof grade: source/dataflow finding, not a locally executed crash reproduction. Existing CI cases prove ordinary claim/release and competing drains, not interruption after a committed claim. Add a CI regression that interrupts at that boundary, rebuilds services against the same database, and proves eventual delivery/visibility without duplicates; also cover the admitted-but-not-marked boundary.

## F. Checks and limits

git diff --check da23eb7..a560297 passed. The review worktree is clean and pinned. No local cargo, just, harness checks or test invocation was run for this re-review, per the CI-first fleet instruction. No mutation test or macOS-only claim is made.

## G. Disposition

Keep the original F001/F002 closed. Delivery owns F003. The prior block is superseded by this single new blocker, not by a claim that the original counterexamples still fail.

## H. Harness improvement

The original review regressions are now committed and have exact-head JUnit receipts. Next encoding: an interrupted-claim/restart fixture alongside them. It must test the delivery handoff, not just another successful store transition.

<details>
<summary>Historical verdict at da23eb7566844c93857ecb11ba58c4da6947c605</summary>

reviewer: block (head da23eb7566844c93857ecb11ba58c4da6947c605)

## A. Verdict

Two reproduced P2 correctness blockers. Both can dispatch one role request to two different holders. No product edits, commits, pushes, production daemon changes or pane interaction.

Re-pinned after the main merge. F001/F002 are unchanged; exact-head CI is now green. This supersedes the earlier fd7bf414 verdict, not its reproduction evidence.

## B. Scope

PR #129, plan 199, AC1-AC7, base 1035c80f1137161db070ce2a06ccaf0e8666d1eb. The original review at fd7bf4141420f132974ece403faa5ae8c64c9982 supplied the runtime reproductions. The replacement worktree fleet-review-129b is clean and frozen at da23eb7566844c93857ecb11ba58c4da6947c605; the original worktree remains at fd7bf414.

The merge resolution preserves s03's rig_with/rig_full signatures and template catalog, introduces rig_inner for the optional hook, and routes draining_rig through None so it still installs the real RoleQueueDrain. with_templates changes only the catalog, not the hook. No new correctness issue in that resolution.

All delivery sources, the role-send HTTP route, daemon composition, store sources and migrations are unchanged between the two heads. In particular, role_send.rs retains blob a05ef0e121330e97425c3038a3d4ccea68cbe6e0 and role_requests.rs retains blob c294d2aeedcf9356b6aca874c45539712a70a7fc. DirectoryService's assign/hook implementation is unchanged too. The prior runtime evidence therefore applies to the unchanged s05 logic; it is not represented as a fresh runtime run of the replacement fixture.

## C. Checks

| Area | Result |
|---|---|
| One-holder routing, C1 mismatch, request/reply/limit receipt | Existing exact-source HTTP test passes |
| Several holders and instance routing | Existing exact-source HTTP test passes |
| Zero holders, FYI queue, PA wake, refused assign, ordered sequential drain | Existing exact-source HTTP test passes |
| Request replay with unchanged holder | Independent positive control passes, one inbox entry |
| Request replay after holder rotation | F001 reproduced |
| Overlapping assign drains across instances | F002 reproduced with a deterministic barrier |
| Project docs | PRD, roadmap and blueprint each validate with 0 errors/0 warnings; build --check reports no drift |
| Replacement-head CI | Run 42138977665: check (22), check (24), rust-lint and rust-test all pass at da23eb7566844c93857ecb11ba58c4da6947c605 |
| Shared seams | The four existing visibility changes are the packet's pub(crate) changes; the new role_send module is public for reuse/tests |
| Cost shape | No new timer, poll or process-launch site. Per-assign drain is one task and up to one delivery per queued request. Live cost verdict remains performance-owned |

## D. Findings

| ID | Severity | Location | Fix |
|---|---|---|---|
| F001 | P2 | crates/daemon/src/delivery/role_send.rs:296-309; crates/daemon/src/http/role_send.rs:171-177 | Establish request-id idempotency before dispatch, and replay the recorded request rather than resolving a new holder |
| F002 | P2 | crates/daemon/src/delivery/role_send.rs:618-646; crates/store/src/role_requests.rs:213-240 | Claim/pin each queued request for one drain before the delivery side effect, with recoverable failure handling |

## E. Reproduction and impact

### F001: replay follows a new holder while the durable request still names the old one

1. Assign pm-001 to pij-holder; send role:pm-001 with msg_id replay-id.
2. Release pij-holder and assign pm-001 to pij-second.
3. Retry the identical send with the identical msg_id.

Both holder inboxes contain the same request. The retry's successful receipt says holder pij-second, but role_request("replay-id") still says pij-holder.

The route never looks up an existing request before delivering. DeliveryService deduplicates by recipient and msg_id, so the new recipient admits the retry. Only afterward does record_delivered encounter E-RS-ROLE-REQUEST-EXISTS, swallow it and return the newly constructed request rather than the stored one.

This breaks ordinary retry safety during role rotation and leaves s06/s12 consuming request metadata that disagrees with the reported recipient. Fix request admission before any delivery: identical retries return the existing request and disposition; conflicting reuse refuses. Preserve the originally admitted holder instead of re-resolving it on a retry.

Observed:

```text
first receipt:  request.id=replay-id holder=pij-holder state=delivered
retry receipt:  request.id=replay-id holder=pij-second state=delivered
durable row:    holder=Some("pij-holder")
holder inbox:  request replay-id
second inbox:  request replay-id
```

### F002: two assign drains deliver before either wins the state transition

Queue one instance-less request to role:reviewer. Assign reviewer/a to pij-holder and reviewer/b to pij-second. Run the production drain function for both assignments, blocking Registry::get until both drains have selected the queued row.

Each drain calls DeliveryService::accept for its different holder before mark_role_request_delivered. Both deliveries are accepted. Only one subsequent UPDATE can change the request, but the losing false result is ignored and cannot undo the second dispatch. The hook spawns independent tasks, so even serialized directory writes do not serialize their drains.

The guard's store comment says it prevents two drains delivering twice; it only prevents two database transitions. Fix with a recoverable, atomic claim or durable handoff before sending. Checking the UPDATE result after delivery is too late. Protect the shared instance-less queue across all instances of a role.

Observed:

```text
outcomes=([("double-drain", Ok(()))], [("double-drain", Ok(()))])
stored_holder=Some("pij-second")
pij-holder inbox: [pij role:reviewer/a request double-drain ...]
pij-second inbox: [pij role:reviewer/b request double-drain ...]
assertion failed: one request reached two assigned holders
left: 2
right: 1
```

## F. Acceptance coverage

AC1/AC3 remain blocked by F001/F002. The committed sequential cases pass but do not cover role rotation or competing drains. AC2/AC4/AC5 have the passing HTTP evidence above. AC6 has static cost-shape evidence only here; performance owns live measurements. AC7 document edits are narrow field changes and validate/render cleanly. The PR contains its Delivers line.

The known #122 F003 wall-clock flake is not changed by this PR. Its submodule was excluded from the scratch review runner; it is not a reason for this block.

## G. Commands and evidence limits

The isolated review package copied fd7bf414's exact directory_http fixture and linked that worktree's path crates. Its only fixture changes retained Services for Registry injection and disabled the unrelated #122 submodule. No product behavior was patched. Runtime results below are carried from that run, supported by the source-identity and merge-resolution checks above.

```text
CARGO_BUILD_JOBS=2 PIJ_SHARED_TARGET=<shared-target> just shared-build --timeout 300s -- test --offline --manifest-path <review-session>/files/role-review/Cargo.toml -p pij-role-review --test role_review review_ -- --nocapture
FAIL as expected: 1 positive control passed, 2 regression assertions failed.
Exact-source compilation completed in 33.35s; tests completed in 1.35s.
An earlier --timeout 120s attempt expired waiting for the shared lock; no compilation started.
```

From that same compiled test executable, each command below ran exactly one test and passed:

```text
role-review-tests a_role_send_reaches_its_one_holder_and_records_the_request --exact --nocapture
role-review-tests several_holders_refuse_by_name_and_an_instance_reaches_only_its_holder --exact --nocapture
role-review-tests no_holder_queues_wakes_the_pa_and_an_assign_drains_in_order --exact --nocapture
```

These six document commands passed:

```text
ddocs validate docs/project/prds/001-pij-community.dd.json
ddocs build docs/project/prds/001-pij-community.dd.json --check
ddocs validate docs/project/roadmaps/001-pij-community.dd.json
ddocs build docs/project/roadmaps/001-pij-community.dd.json --check
ddocs validate docs/project/blueprints/001-pij-community.dd.json
ddocs build docs/project/blueprints/001-pij-community.dd.json --check
```

git diff --check passed, including the replacement-head delta. harness boot reported ready:false in both worktrees because node_modules is absent. On fd7bf414, harness checks --quick failed six JS/tooling stages; Copilot native contracts passed, attribution was unavailable, Rust/smoke were skipped. C25 forbids the dependency-link workaround. The full local composite gate was not run on the loaded fleet.

GitHub initially exposed zero check runs, zero workflow runs and zero commit statuses at fd7bf414. On the replacement head da23eb7566844c93857ecb11ba58c4da6947c605, gh pr view and gh run list confirm completed successful run 42138977665 with check (22), check (24), rust-lint and rust-test all green. The #122 timing test was not changed by the merge. No additional local Cargo run was needed for the test-rig-only conflict resolution.

## H. Handover

Delivery owns F001/F002. Keep s05 and dependent delivery lanes unlanded until both reproductions pass on a new frozen head. Re-run the positive control and the three committed HTTP cases too.

magicWand: commit deterministic request-replay and competing-drain fixtures, so role rotation and CAS-after-send mistakes fail without another review. The highest-value harness improvement is those two regressions, not another store-only assertion. Secondary friction: review preflight should report C25-safe tooling and exact-head CI availability rather than suggesting a forbidden link.

</details>

</details>
