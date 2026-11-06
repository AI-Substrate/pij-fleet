# PR #135 repair verdict

**block** at `3ecec77f46ac335e6fe141ecf75baf5637378e7b`. F002 and F003 close. The ordinary fresh-send cases of F001 are fixed, but its lifecycle fence remains incomplete at queue re-entry and cancellation boundaries.

## Scope and proof

Reviewed only `e60be548..3ecec77`: fix `f66185e3bb809ff8a30b2f01ed58d662cdcde3a4` and the two main merges, including the merged batch-assign refusal. The detached `~/pi-hacking/review-135-3ecec77` worktree stayed clean and pinned. Main's independently reviewed #133/#138/#146/#147 features are not re-reviewed wholesale.

[Exact-head CI 42216912927](https://github.com/AI-Substrate/pij-fleet/actions/runs/42216912927) is green in all four jobs. Rust ran **2,005 passed, five skipped**. Downloaded JUnit confirms all three new regressions executed:

- `reshape::sends_and_retirement_share_one_admission_gate`
- `reshape::an_unfinished_withdrawal_is_finished_by_the_same_command`
- `reshape::printed_repair_commands_keep_the_project_and_instance`

The existing stopped-daemon recovery, ordered drain and released-holder controls also executed. Those passing component cases do not exercise retirement between an in-flight claim and its requeue. Source/dataflow counterexamples below were not locally reproduced. No local builds, test suites or probes ran.

## Closed findings and merge check

**F002 closed:** HTTP reshape holds FYIs first, with stable request-derived IDs, then performs withdrawal. A retired role with requests left queued can resume the same drop operation without another writer commit or changelog row (`data.resumed`, null commit). The regression seeds the interrupted state, proves one event/FYI and unchanged history, and checks refusal once nothing remains. It is recovery-state proof, not an injected failure of the first command.

**F003 closed:** held-role releases, queued-instance assigns/drop actions and F19's edit retain project and instance. The two-project regression replays the emitted actions from A against B, drains the named-instance request, and proves A's head unchanged.

**Batch merge guard preserved:** `DirectoryService::assign` delegates to `assign_many`; its writer-locked loop calls `refuse_retired` for every distinct role before constructing/writing the batch (`community/directory.rs:1029-1045`). A retired role cannot be hidden behind another active role in the batch. This mixed-batch conclusion is source proof; the existing retired single-assign test exercises the shared path.

## Residual F001, P2: own the complete admission boundary

### A. Failed/incomplete drains can requeue after retirement

The preflight at `community/directory.rs:668-690` calls `queued_for_role`, whose SQL considers only `state = 'queued'` (`store/role_requests.rs:273-285`). It ignores a request already claimed as `delivering`.

A valid ordering is:

1. A drain claims Q while its holder is live and assigned.
2. The holder is released or closed; its holding ends.
3. Retire sees no queued request and no open holding, and commits.
4. Q's dispatch then fails. `delivery/role_send.rs:786-790` calls `release_role_request_claim`, which unconditionally changes that claimed row back to queued (`store/role_requests.rs:655-664`), without the gate or a standing check.

Q is now queued for a retired role, where a new assignment refuses. A restart has the same hole: recovery returns an unadmitted delivering row to queued at `store/role_requests.rs:687-713`, without consulting role standing. The new retry command offers manual cleanup afterwards, but does not prevent this silent stranding.

**Required pressure:** pause an in-flight drain after its claim, end the holding, retire, then exercise both failed dispatch and boot recovery. The result must not be a queued request behind a retired role. Fence queue re-entry as well as fresh inserts, or conservatively prevent retirement while unsettled work can still requeue. Also coordinate queued-to-delivering claims with the tell/withdraw snapshot.

### B. The guard does not follow persistence that outlives its waiter

The send owns its gate in the outer `route` future (`delivery/role_send.rs:194-198,268-275`). Its write runs in an independent task: `record_role_request` uses `owned_write`, whose explicit contract says dropping the waiter does not cancel the transaction (`store/role_requests.rs:173-180`; `store/migrate.rs:43-56`).

Pause that owned insert, cancel the waiting send, retire on the now-free gate while the queue is empty, then let the insert commit. The role is retired with a new queued row.

Retirement has the symmetric window: its guard remains in the outer reshape future, while `commit` launches `spawn_blocking` without capturing it (`community/directory.rs:660,700-768,1486-1501`). Cancel the waiter while that blocking job is waiting for the writer lock; a new send can acquire the released gate and queue against the still-active role, after which the detached writer retires it.

**Required pressure:** abort waiting futures after their persistence work has been admitted, hold/release explicit barriers, and verify no mutation escapes its lifecycle fence. The guard must remain owned by the work that can still mutate, not just by the caller awaiting it. No age threshold or retention decision is needed.

## Limits and handover

The shared gate and its `without_hook` clone correctly serialize the healthy fresh-send/reshape paths. The current gate regression uses the shared lock and timed waits to exercise those orders; it does not cover the re-entry or aborted-waiter schedules above.

The gate adds no OS launch, timer or poll; it is request-driven, not per-seat/per-tick work. Cost/resume budgets and security conclusions remain independently owned.

Any refusal while in-flight work can still requeue would be a one-directional safety brake: removing it permits more retirement. Explicit `--drop-queued` remains the deletion authorization; request age is not a deletion policy.

PM request: `01a1b0d2-7618-71a3-a8f0-1ceca4a54a89`. Re-review only the next frozen repair delta with the two missing schedules covered. No product files were edited.

**Publication:** [pinned verdict comment](https://github.com/AI-Substrate/pij-fleet/pull/135#issuecomment-6685088600) verified with both residual schedules present. Its API-derived receipt was appended once to `roles/reviewer/outputs/LOG.md:132`, without rewriting the ledger. Role memory was written through the verb in governance commit `9e8291d`.
