# PR #130 correctness review

## Merge-only re-review at 5d22029

**pass-with-notes** at `5d2202949705b7345fd78177784ac29a4bd5282e`. No new correctness blocker. The `bfa8984` dispositions stand. Scope is merge `9156cfa` and test-only `5d22029`, not independent review of incoming main.

The fresh detached checkout is clean and pinned correctly. Merge parents are `bfa8984` and `2c6fa1e`. The remerge diff preserves `Built.routes` and `Built.batch`, routes and lineage leaves, routes and role-send HTTP modules, the liveness fake and optional real queue-drain hook, both whole test blocks, and both API/SKILL entries. Comparing directory tests against the second parent confirms the incoming role-send/recovery tests remain, apart from the explicitly requested rider assertion adjustment.

### Batch routes addition

`hold` still returns directly into `assign_batch`, so it cannot also execute the single-assign routes callback. Batch input is validated and `assign_many` must succeed before any new routes delivery. The callback then uses the existing `Trigger::Assign`, explicit project, fresh FYI identity and unchanged `routes::deliver` implementation.

The per-seat result map guards delivery before inserting each outcome. It suppresses repeated recipients even in non-adjacent A/B/A input; correctness does not depend on the preceding adjacent-only `Vec::dedup`. Each distinct seat gets one delivery attempt and one `data.routes[seat]` outcome, regardless of how many holdings it acquired. The existing helper holds one FYI from `pij-bg`, never a wake, and reports a failed hold without undoing the assignment.

`admit_control` is verbatim unchanged from `bfa8984`: the compact callback remains after successful `accept_control`, with key `routes-after-compact:<seat>:<msg_id>`. Held memory turns still call this shared helper. The original same-recipient replay and two-recipient tests remain intact and pass.

### Evidence and non-blocking limits

Exact-head CI run [42219957763](https://github.com/AI-Substrate/pij-fleet/actions/runs/42219957763) passes all four jobs. Downloaded JUnit records **2,006 tests, zero failures, errors or skips**, including batch assignment, route regeneration/demand/triggers, both compact identity regressions, queue draining and interrupted-claim recovery. Node 22 records 2,200 passed and six skipped, including all 15 shared-build tests. The ddocs stage reports UNAVAILABLE, not document validation.

The drain test still compares the exact oldest request prefix; it now permits an optional FYI suffix containing the routing-table marker. Its existing request state/order/event checks remain. The suffix check is marker-based, not an exhaustive parser of every rider.

**Testing note, non-blocking:** the existing batch test executes the new path but does not assert `data.routes` or per-seat FYI counts. Once-per-distinct-seat cardinality is source-backed here, not a dedicated CI regression claim. The highest-leverage harness follow-up is an A/B/A batch with unique holdings, asserting two receipt keys and exactly one held routing FYI per seat, plus no FYI from a refused batch.

The approved compact-context-consumption proof ceiling below remains unchanged. Security and performance retain their independent verdicts. Cost shape is one table build/hold per distinct seat per successful batch; no timer, poll or process-launch site is added.

Read-only commands: exact HEAD/status and first-parent log, `git show --diff-merges=remerge 9156cfa`, per-parent/test diffs, source reads, `gh pr view 130`, `gh run view 42219957763`, `gh run download ... --name junit-linux`, and the check (22) log. Artifacts are retained under `<reviewer-session-files>/pr130-5d22029-{remerge.diff,tests-v-main.diff,ci/junit.xml}`. No local builds, test suites, mutations, live-daemon operations, product edits, commits, pushes or checkout movement. Final publication check is recorded with the verdict delivery.

---

# Historical merge-only verdict at bfa8984

**pass-with-notes** at `bfa89846a1cb009b3094e263f14f85ce38a6cc9a`. The `8bfba9d` dispositions stand; no new correctness finding in merge `9bf1afc` and test-only `bfa8984`.

The fresh detached `review-130-bfa8984` checkout is clean and pinned correctly. Review used the merge's remerge diff to isolate the eleven conflict resolutions rather than re-review incoming main. Directory help, the API table/sections, SKILL rows, source/rendered project states and both sides' changelog entries are retained. The directory-service merge preserves both memory_add and the route-regenerating writer.

The compact hook is correctly located in shared `admit_control`, **after** `accept_control` succeeds. Immediate compacts call that helper from `send_control`; held compacts call it from memory-turn `proceed`. The held HTTP reply itself does not emit a routes FYI, and there is no duplicate hook left in `send_control`. This is post-admission placement, not a claim of terminal harness execution.

The derived key remains exactly `routes-after-compact:<seat>:<msg_id>`: target now comes from the control's `Msg.to`, with the same recipient/message identity reviewed at `8bfba9d`. Existing FYI dedupe therefore retains same-seat replay suppression after claim and separation between recipients sharing a control ID.

The three changed HTTP tests now wait for the required **count** of that seat's memory.turn completion events before inspecting control/FYI state. The provider appends that event after the shared admission helper returns, so this is a causal wait for the moved routes callback, not reuse of an earlier latest-event snapshot. The replay still checks one control and one compact table; the two-recipient test checks one table for each recipient across repeats.

Exact-head CI run [42188192367](https://github.com/AI-Substrate/pij-fleet/actions/runs/42188192367) passes all four jobs. Downloaded JUnit has **1,967 tests, zero failures/errors**, including the three compact routes regressions and held parent/self compact cases. No local builds, tests, mutations or probes; no build lock acquired, product edits, commits, pushes or checkout movement.

**Merge-state qualification:** despite the handoff's earlier MERGEABLE reading, GitHub currently reports **DIRTY / CONFLICTING** at this unchanged head. Existing head checks remain green. Reconcile against current main before merge; this verdict does not cover a later resolution.

The original compact-context-consumption proof ceiling remains a non-blocking note under the approved held-FYI definition. Independent authorization and cost verdicts remain security/performance-owned. The move adds no new routes job, poll or process launch; it reuses the same keyed FYI hold at the correct shared seam.

**magicWand fulfilled:** tests now wait for the composed memory-turn/admission boundary before asserting the routes invariant.

---

# Historical final-delta verdict at 8bfba9d

## Final verdict

**pass-with-notes** at `8bfba9de1206a0f781bd853827df38abc961ca08`. Supersedes the block at `9d2a746` and the earlier block at `dcc3e526`, preserved below.

Reviewed only the five-file `9d2a746..8bfba9d` delta (+92/-11), including the sanitizing commit `1623200` and F002 repair. The fresh detached `review-130-8bfba9d` checkout is clean and pinned correctly. **No correctness blocker remains.**

### Findings closed

- **F001 remains closed:** the compact table's stable FYI key still suppresses same-recipient replay both while held and after claim. The original three-attempt regression passes at this head.
- **F002 closed:** the key is now `routes-after-compact:<seat>:<msg_id>`, preserving the control's recipient scope when mapped into the global FYI key. Two holders sharing a compact message ID get distinct table IDs, while repeating either request retains its original ID. The new two-recipient/two-attempt HTTP regression passes. The fixture's added dormant pane enables it to be a compact target while its unchanged absent process identity still supports the revive test; that existing trigger test also passes.
- **Sanitizing note addressed:** table cells remove control characters and the named bidi ranges, retain whitespace normalization and pipe escaping, and overlap-warning paths now go through the same helper. The updated cell and overlap tests pass. No separate security verdict is implied.

### Exact-head evidence

CI run [42149295542](https://github.com/AI-Substrate/pij-fleet/actions/runs/42149295542) is successful at the exact final SHA: check (22), check (24), rust-lint and rust-test. GitHub reports CLEAN.

Downloaded `junit-linux` records **1,901 tests, zero failures/errors**, including:

- `reviewer_compact_request_replay_does_not_duplicate_routes`
- `one_compact_msg_id_to_two_seats_gives_each_its_own_table_once`
- `assign_compact_and_revive_each_send_a_holder_the_table_once`
- `community::routes::tests::cells_stay_one_line_with_pipes_escaped`
- `community::routes::tests::overlapping_claims_from_two_roles_are_named`

Commands used: read-only git HEAD/status/diff and source reads; `gh pr view 130 --json headRefOid,mergeStateStatus,statusCheckRollup`; `gh run view 42149295542 --json headSha,status,conclusion,jobs`; `gh run download 42149295542 --name junit-linux --dir <reviewer-session-files>/pr130-8bfba9d-ci`. CI artifact download succeeded; no tests were executed locally. CI covers this delta, so no local check or build slot was needed. No product edits, commits, pushes or checkout movement.

### Remaining non-blocking notes

The original compact-consumption proof ceiling is unchanged: holding the FYI does not itself prove Copilot receives fresh context after compaction. This remains a note under pm-001's approved held-FYI definition of delivery, not a reopened correctness blocker. The independent deployment-cost reading remains owned by performance/the merger.

The delta adds no timer, poll or process launch. Sanitizing is in-process; deduplication reuses the existing FYI insert and persistent rows.

**magicWand fulfilled:** CI now retains both identity-scope regressions: one recipient replayed before/after claim, and two recipients sharing one compact ID.

---

# Historical repair verdict at 9d2a746

## Current verdict

**block** at `9d2a746b222f53e86383d6739db01a3ec18efa5a`. This supersedes the verdict at `dcc3e526043b2aa3206eae0ea036876183fb9d99`, preserved below.

Reviewed only the seven-file `dcc3e526..9d2a746` delta (+129/-11), its dependent FYI/control dedupe contracts and exact-head CI. The fresh detached `review-130-9d2a746` checkout is clean and pinned correctly. No local builds, tests, mutations, probes or harness gates were run under the CI-only backoff; no lock acquired.

## F001 closed; F002 introduced by the repair

**F001, same-recipient replay: closed.** The compact callback now derives a stable FYI ID; assign, revive and prime notices retain fresh IDs. The FYI primary key persists across held, delivered and dropped states, and claiming updates state rather than deleting the row. The adopted regression covers the initial compact, same-seat replay while held, and replay after marking the FYIs delivered. It passes in exact-head CI.

**F002, P2, blocking: the derived ID collapses different recipients.**

Locations: `crates/daemon/src/http/mod.rs:2061`; `crates/daemon/src/http/routes.rs:243-248`; supporting store contracts in `crates/store/src/queue.rs:1221-1234` and migrations `0027_fyi_origin.sql`, `0028_message_origin.sql`.

The new key is only `routes-after-compact:<msg_id>`. All these FYIs are local (`from_machine: None`), so the FYI store's global `(origin, id)` primary key sees the same key for every recipient. It uses `INSERT OR IGNORE` and does not change the first row's recipient when the insertion is ignored.

Controls do not have that scope: delivery dedupe uses recipient-specific job kinds and the delivered ledger uses the recipient. The same caller-chosen message ID can therefore identify separate compact operations for two seats.

Concrete source trace, requiring no timing assumption:

1. Compact role holder A with `msg_id = "shared-compact"`. Its control is admitted and FYI `("", "routes-after-compact:shared-compact")` is held for A.
2. Compact role holder B with the same `msg_id`. B's control is independently admitted because its delivery kind/recipient differ.
3. B's table uses the same global FYI key, so its insertion is ignored. B gets no table, yet `hold_fyi` returns success. This remains true after A's FYI is claimed because the original row persists.

This is a new AC2 regression caused by narrowing the derived key more than the source operation's identity. The single-recipient regression cannot expose it.

**Required repair:** include the recipient in the stable derived identity, using an unambiguous encoding, while preserving the original same-recipient replay behavior. Extend CI with two holder recipients sharing one compact message ID: one control and one table per recipient, with neither recipient's replay adding another table before or after claim.

Proof grade: direct source/SQL analysis, not a local reproduction.

## Exact-head evidence and unchanged notes

- `gh pr view 130 --json headRefOid,mergeStateStatus,statusCheckRollup`: exact `9d2a746...`, CLEAN, all four checks successful.
- `gh run view 42147270960 --json headSha,status,conclusion,jobs`: exact head, completed, success.
- Downloaded `junit-linux` from that run: 1,900 tests, zero failures/errors. `reviewer_compact_request_replay_does_not_duplicate_routes`, the ordinary assign/compact/revive test and on-demand routes test pass.
- No local validation was executed. CI JUnit is retained under `<reviewer-session-files>/pr130-9d2a746-ci/junit.xml`.

The new API/evidence text accurately describes the same-recipient repair but must reflect the recipient-scoped key after F002 is fixed. The compact consumption proof ceiling from the original review remains non-blocking under the approved held-FYI definition. Security's optional control-character sanitizing was not taken; it remains a separately owned note, not a new correctness finding here.

No timer, poll or process launch was added by this delta. Replay still performs the existing reads/hold attempt; an ignored FYI insert adds no held event. No new steady-state cost surface.

**Handover:** original F001 is closed, but keep the PR blocked for new F002 and return a repaired frozen head. No reviewer build window is needed.

**Final pin qualification:** GitHub advanced to `162320050313a17308e62827ed6fc3332d05cb53` during this review. This verdict and its green CI evidence cover **only 9d2a746**, not that later head. The detached review worktree was not moved.

**magicWand:** test the conversion between a recipient-scoped control identity and the globally scoped FYI key, not only repeated readings of one recipient.

---

# Historical verdict at dcc3e526

## A. Verdict

**block** at `dcc3e526043b2aa3206eae0ea036876183fb9d99`.

F001 is a reproduced P2: retrying one compact request holds another routing-table FYI even though delivery deduplicates the compact itself. This violates AC2/AC4's one delivery per trigger. Security and performance verdicts remain separately owned.

## B. Scope and basis

Reviewed all changed implementation seams, the new routing module and HTTP tests, plan 202 AC1-AC5, evidence, project-state changes and CLI/API/skill documentation. Worktree HEAD and GitHub head match the pinned SHA. No product edits, commits, pushes, production-daemon mutations or pane input.

GitHub reports CLEAN and four successful jobs for exact-head run [42139627097](https://github.com/AI-Substrate/pij-fleet/actions/runs/42139627097): check (22), check (24), rust-lint and rust-test.

## C. Checklist

| Requirement | Result |
|---|---|
| Scopes become deterministic path/rule rows; overlaps warn | Implemented; conservative literal-prefix overlap semantics are documented |
| ROUTES.md rides the role writer's commit | Implemented under the existing worktree lock, including stale-file replacement |
| Holder names stay out of ROUTES.md | Implemented; only seat-facing rendering joins current holdings |
| Assign, compact, revive and demand | Ordinary paths covered; compact replay fails F001; on demand returns the table in the reply rather than holding an FYI |
| Changed rows notify the prime once; holder changes do not | Current add path implements this; assign/release do not call the prime notifier |
| Minimal assign/control/revive seams | Yes; no new loop, timer or repeated process launch |
| s08 complete, req-11/c-routes shipped, Delivers line | Present; project validation/render checks pass |

## D. Findings

| ID | Priority | Location | Finding |
|---|---|---|---|
| F001 | P2, blocking | `crates/daemon/src/http/mod.rs:2056-2060`; `crates/daemon/src/http/routes.rs:227-235` | Every successful compact receipt, including a deduplicated replay, creates a fresh routes FYI |

## E. F001 reproduction and repair contract

The new callback sits after `accept_control`, outside its request-id deduplication. Reposting the same authenticated native `/v1/send` body with `command: "compact"` and the same `msg_id` returns a successful existing-control receipt. The callback nevertheless calls `deliver` again; `hold` allocates a new message ID each time, so the FYI store cannot deduplicate it.

Independent scratch HTTP regression uses the committed directory test rig with a real SQLite store and scratch governance repository. It first adds a scoped role and assigns the recipient, then sends one identical compact request twice. It queries both durable jobs and FYIs:

```text
attempt=1 controls=1 held_fyis=2
attempt=2 controls=1 held_fyis=3
assertion failed: one assign FYI and one compact FYI, even on replay
```

The first request is the positive control: exactly one compact row and one additional table. The replay still has only one compact row but now two compact tables. No execution, timing assumption, live fleet interaction or known #122 same-second test is involved.

**Required repair:** bind the routes side effect durably to the compact request identity so retries cannot create another table. Checking only whether the receipt is queued is insufficient: both the original and the deduplicated pending replay return successful queued receipts. Retain the regression and cover replay after FYI claim as well as while held.

## F. Coverage and remaining proof ceiling

AC1 and the ordinary AC3 path are supported by the implementation and committed tests. AC2/AC4 fail the replay case above. AC5's requested document flips and Delivers line are present; project documents validate and their generated views are current. Do not treat this review as closing the remaining exact-head verdict requirements.

**Copilot compact limitation:** the table is held at control admission, not at a proven post-compact boundary. Copilot's `onUserPromptSubmitted` hook unconditionally claims held FYIs; whether a typed `/compact` invokes it remains unproven. If it does, the table can enter the context being compacted instead of being freshly supplied afterward. Person-typed and harness-initiated compactions also bypass this daemon trigger entirely. The existing HTTP test proves holding, not model consumption.

This matters to the advertised first-post-compact-turn guarantee, which is not established. Under pm-001's approved held-FYI definition of delivery, it is a separate, non-blocking evidence qualification, not a second reproduced blocker. A native harness canary observing claim and compaction boundaries would settle it. OMP's compaction latch supports its ordinary execution path but is not evidence about Copilot.

There is no new steady-state loop, poll or timer. Deliveries do in-process reads and one held FYI per ordinary trigger; F001 makes that count grow with request replays. The live after-deploy cost reading remains explicitly owed to the performance/merge owner.

## G. Validation

- `gh pr view 130 --json headRefOid,mergeStateStatus,statusCheckRollup`: exact pinned head, CLEAN, four successful checks. `gh run view 42139627097 --json headSha,status,conclusion,jobs`: same SHA, success.
- `git diff --check origin/main...HEAD`: pass.
- Canonical-checkout `tsx harness/scripts/ddocs-check.ts`, run from this worktree: pass, seven documents validate and rendered views are current.
- Same `tsx` runner for `harness/scripts/local-path-check.ts` and `harness/scripts/lockfile-allowlist.ts`: pass; 223 lockfile sources allowlisted.
- `harness plan validate docs/plans/202-routing-table/plan.dd.json`: pass, no errors/warnings/contradictions; six completion items remain open.
- Immutable pinned-source snapshot, only reviewer test added: `PIJ_SHARED_TARGET=<warm-target> just shared-build -- test --locked -p pij-daemon --test directory_http reviewer_compact_request_replay -- --nocapture`: expected regression failure, reproduced F001.
- Same immutable snapshot and shared-build wrapper: `test --locked -p pij-daemon --test directory_http routes -- --skip reviewer_compact_request_replay --nocapture`, then `test --locked -p pij-daemon --test directory_http assign_compact_and_revive_each_send_a_holder_the_table_once -- --exact --nocapture`: pass, all three committed routing HTTP controls.
- Guard mutation (disable only the compact routes callback in the scratch snapshot): **deferred because of the fleet backoff**. The waiting wrapper terminated with exit 143 before acquiring the shared target lock or running cargo. No mutation result claimed. Scratch source was restored byte-for-byte; no mutated build artifact was produced. No further local builds, tests or probes will run under the backoff.
- `harness boot`: not ready because this worktree intentionally has no node_modules. `harness checks` was attempted and stopped when it entered an unscoped full Rust build; it is not a pass. `harness checks --quick`: six local tooling/dependency failures, Copilot-native pass, Rust/smoke skipped, attribution unavailable. No dependencies installed or symlinked. Exact-head CI supplies the full TS/Rust gate evidence, not a claim that the local composite gate passed.

Scratch sources and receipts are retained under `<reviewer-session-files>/pr130-probe/`, `pr130-replay-proof.txt`, `pr130-baseline-proof.txt` and `pr130-trigger-mutation.txt`. Reusable reproducer: [pr-130-replay-regression.rs](pr-130-replay-regression.rs). The review checkout remains frozen and unchanged. The interrupted local full-gate build's 8.6 GiB output was removed after verifying no build used that target; the shared target was not removed.

## H. Handover and harness feedback

Keep the PR blocked for F001; send the repaired exact head for re-review. No finding is raised for the unrelated #122 same-second flake.

**magicWand:** make the compact routes test replay a caller-chosen request ID and independently count control jobs and table FYIs. The retained regression encodes that missing oracle; the repair should bring it into the committed suite.

**difficulties:** the supplied warm target no longer existed, so the focused build used the extant fleet-190 target through `just shared-build`, with `-p pij-daemon`, two build jobs and incremental disabled. Missing JS dependencies limit local composite checks. Admission receipts cannot prove post-compaction context delivery.
