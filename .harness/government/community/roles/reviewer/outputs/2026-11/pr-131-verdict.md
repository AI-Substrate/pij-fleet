# PR #131 correctness review

## A. Verdict

**block**, exact head `a00a4cfbb287efb4bc60b292b1b168404a94677e`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/131#issuecomment-6673990421

Three P2 findings below prevent approval, based on source review plus green exact-head CI. F004 is withdrawn following the prime's design ruling, relayed by pm-001 on 2026-11-06: delivered prints `delivered_by` and the plan cell, while PRs flip their own states. Additional HTTP proof is **deferred because of the fleet backoff**, not a prerequisite to this blocking verdict. PM confirmed that the waiting builds were intentionally stopped to protect the shared machine; no reviewer build window is available. Four unexecuted regression candidates are supplied in `pr-131-regression-tests.rs`. Findings are source-proven, not claimed runtime reproductions.

## B. Scope

Plan 196, roadmap 001 s11, AC1–AC9; all 46 changed paths, relevant existing integration boundaries, C23 and the reviewer standing packet. The detached review worktree remains at the requested SHA, with no tracked edits, commits, pushes, deployment, dependency installation or daemon restart.

## C. Checks

The store allocates under `BEGIN IMMEDIATE`, retaining released claims in its maximum and enforcing the unique project/kind/number key. Tombstone release is inside `publish_tombstone`'s existing transaction. Delivery records, claim releases and one spine event commit together. The gh adapter is injected, resolved once, launched by absolute path and counted. The CLI, shim, card, golden fixture and skill coverage are wired.

`Delivers: none` is a reasonable explicit no-row declaration and preserves the missing-line refusal. It is not a blocker. Printing rather than applying edits is correct. The prime has settled design call (5): printing `delivered_by` and the plan cell is right; PRs keep flipping their own states, so `pij delivered` does not print state flips.

## D. Findings

| Id | Priority | Location | Finding |
|---|---|---|---|
| F001 | P2 | `crates/daemon/src/http/claims.rs:515-523` | An unreadable checkout is treated as an empty number namespace. |
| F002 | P2 | `crates/daemon/src/http/claims.rs:478-486` | Pending delivery edits overwrite receipts from earlier merged PRs. |
| F003 | P2 | `crates/daemon/src/http/claims.rs:468-486` | An unreadable or malformed document produces a destructive replacement `delivered_by` edit. |
| F004 | withdrawn | `crates/daemon/src/http/claims.rs:483-507` | The prime confirmed that delivered does not print state flips; no change owed. |
| N001 | P3 | `crates/daemon/src/http/claims.rs:373-398` | A replay reports the current PM as notified, even when only a different earlier PM has the delivery. |

## E. Details and closure criteria

**F001. Do not allocate when the checkout floor could not be established.** Both directory-open and directory-entry errors are discarded with `.ok()`, and an all-error scan becomes zero. With an empty claims table, `docs/plans/001-existing` and `archive/195-b` already present, and `docs/plans` temporarily unreadable, the request allocates plan 001 successfully. This duplicates an existing file number despite the database's perfectly working uniqueness constraint. Distinguish legitimately absent optional folders from failed reads; return an actionable refusal before recording a claim when the scan is incomplete. Add a real permission-error control that first asserts the directory is actually unreadable.

**F002. Include already recorded deliveries when constructing the next docs edit.** The current value comes only from the checkout, and `delivered_by` appends only the current PR; the delivery store is never consulted for that address. Starting at `#110`, merge/record #153 and #154 before the PM's next docs PR. The commands set `#110, #153`, then `#110, #154`. Applying them in order loses #153, although its delivery is still recorded in the store. This is the normal deferred-docs workflow, not an exotic race, and contradicts AC5's multi-PR preservation promise. Produce an accumulated edit from the existing cell plus persisted deliveries for the same project/address, accounting for the current PR; test two deliveries without applying the first command.

**F003. Unknown is not empty.** Reading or parsing a document can fail, yet `edits` merely adds a note and passes `None` to `delivered_by`. A requirement whose real value is `#110` then receives the executable command `ddocs set '<requirement>/delivered_by' '#150'`. Once access or valid JSON is restored, running the proposed edit erases #110. The warning does not make that replacement safe. Refuse or explicitly omit the unsafe edit with actionable diagnostics; do not emit an empty-cell replacement unless the document and addressed row were read successfully. The supplied probe permits either a refusal or a safe omission.

**F004 withdrawn.** On 2026-11-06, pm-001 relayed the prime's settled design call (5): “Printing delivered_by and the plan cell is right; PRs keep flipping their own states, so `pij delivered` doesn't print state flips.” This resolves the apparent conflict with C23's earlier wording. The current implementation is correct on this point; no state-edit change or regression test is owed.

**N001. Preserve factual retry receipts.** First record a PR with `--pm pij-test-pm`, then replay with `--pm pij-test-other`. `ON CONFLICT DO NOTHING` keeps the original PM, appends no event and leaves the other card empty, but `data.pm` and the human line claim `pij-test-other`'s card “was told the first time”. The store already returns `earlier_pm`; use the persisted recipient when describing a no-op replay, or explicitly distinguish requested and actual recipient. Do not transfer or resend a delivery just to fix the wording.

## F. Acceptance coverage

| AC | Disposition |
|---|---|
| AC1 | Normal floors/widths/kinds covered; F001 blocks the incomplete-scan case. |
| AC2 | Ten real concurrent store claims are distinct/consecutive; cross-project and unique-key controls pass. |
| AC3 | Owner/prime/manual release, burned numbers, tombstone release and delivered claim release are covered by passing store tests and source review. |
| AC4 | Existing merged-only, missing-line and same-input idempotency tests are in green exact-head CI; no new poll or webhook. |
| AC5 | No project-write path is added; F002/F003 block the append-preservation claim. No state-flip output is required under the prime's settled design call. |
| AC6 | Card field, indexed read, holder selection, fallback and event are wired; N001 qualifies changed-PM retry reporting. |
| AC7 | Procedures and both READMEs use `pij claim`; PR template has the `Delivers:` line. |
| AC8 | No loop, poll, timer or sweep added. Claims launch nothing; delivery has one counted gh launch per call. Live cost verdict/deploy receipts remain performance/PM-owned. |
| AC9 | API/refusals/shim/skill changes present; s11 complete with plan cell, req-20/21 and both components shipped; seven deterministic documents validate and render clean. |

## G. Commands and evidence

The verdict rests on source review and exact-head CI. The historical local receipts below were collected before this seat received the backoff instruction; they are not a request for further local proof. All Cargo commands used the assigned shared target and `just shared-build`, with `CARGO_INCREMENTAL=0` enforced by that recipe. PM subsequently confirmed that the interrupted waiting builds were stopped deliberately as a one-directional safety brake under the prime's backoff, not because of a PR defect. Additional HTTP proof is deferred to CI. Since receiving the backoff, no local builds, test runs, probes or scripts have been run.

| Command | Result |
|---|---|
| `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target just shared-build -- test -p pij-core --lib claims:: --then test -p pij-store --test claims --then test -p pij-daemon --test claims_review -- --nocapture` | Core 5/5 and store 5/5 passed. First HTTP test-harness compile failed because `include!` cannot consume inner doc comments; three comments were corrected in the scratch test fixture only. No PR defect is attributed to that reviewer error. |
| `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target just shared-build -- test -p pij-daemon --test claims_review -- --nocapture` | Stopped during the shared-lock wait under the prime's fleet backoff; no compile/tests. HTTP proof deferred to CI. |
| `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target just shared-build --timeout 10 -- test -p pij-daemon --test claims_review -- --nocapture` | Again stopped during the lock wait; no compile/tests. PM clarified the intentional safety stop and that reviewers have no build window. No lock was cleared or another seat interrupted by this reviewer. |
| `python <reviewer-output>/2026-11/pr-131-claim-key-control.py` from the review root | Duplicate claim key rejected; same number in another project/kind accepted. Removing only the claims primary key makes the same duplicate oracle fail: mutation detected. |
| `harness boot --json` | Advisory `ready:false`: local Vitest/tsx absent. No install or dependency-link workaround. |
| `harness checks --quick --json` | Local gate not green: six tooling-limited stages; native Copilot contracts pass; Rust/smoke intentionally skipped. Not represented as a successful full local gate. |
| `/Users/jordanknight/pi-hacking/pij/node_modules/.bin/tsx harness/scripts/ddocs-check.ts` from the review root | Passed: seven documents validate clean and their `.dd.md` are current. No generated files written. |
| `/Users/jordanknight/pi-hacking/pij/node_modules/.bin/tsx harness/scripts/local-path-check.ts` | Passed. |
| `/Users/jordanknight/pi-hacking/pij/node_modules/.bin/tsx harness/scripts/lockfile-allowlist.ts` | Passed: 223 sources allowlisted. |
| `/Users/jordanknight/pi-hacking/pij/node_modules/.bin/tsx harness/scripts/pij-commit-trailers.ts --json` | Passed: 13 commits, no findings or unverified entries. |
| `git diff --check origin/main...HEAD` | Passed. |
| `gh run view 42140757570 --json headSha,status,conclusion,jobs` | Exact requested head; completed success for Node 22, Node 24, rust-lint and rust-test. |
| `gh pr view 131 --json headRefOid,mergeStateStatus,statusCheckRollup` | Requested head unchanged; CLEAN, all four checks successful. |

The supplied HTTP tests are regression candidates, not executed failing receipts. Their execution is deferred because of the backoff and must come from CI, not a local reviewer build. No private-daemon run, live-fleet measurement or complete local smoke gate is claimed.

## H. Handover and harness gift

Coder: close F001–F003 with focused CI regressions; N001 is a small receipt-correctness follow-up. F004 is withdrawn, with no change owed. Freeze the repair head and cancel any earlier push instructions before re-review. Review remains CI-only until the prime lifts the backoff.

**magicWand:** add a table-driven claims/delivery fixture matrix covering unreadable inputs, two pending docs receipts and a changed recipient on retry. It moves the three current inference gaps into deterministic CI. `pr-131-regression-tests.rs` supplies four bounded cases for adoption; the withdrawn state-edit case has been removed.

**difficulties:** local checks used incompatible ambient tools after boot had already established missing worktree dependencies. PM clarified that the interrupted shared-build waits were intentional safety stops, not unexplained harness failures. The useful improvement is an honest CI-only review-readiness/unavailable-sensor path and a durable cancellation reason, never a lock bypass or another local run. Observations were recorded before receipt of the backoff; no product/harness code was changed by this read-only reviewer.
