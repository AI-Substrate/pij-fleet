# PR #131 repair review

## A. Verdict

**pass-with-notes**, exact reviewed head `09653d32a47224a61f0e2957b3862001b869e218`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/131#issuecomment-6674732467

F001, F002, F003 and N001 are closed. F004 remains withdrawn under the prime's ruling. No correctness changes are owed at this reviewed head.

**Head qualification:** GitHub advanced to `93ca2b1c3dc80d3ea609438e9864d9593b7e35f5` during finalization and reports BLOCKED. This verdict and its green CI apply only to `09653d3`, not that newer head. PM was notified. The detached `review-131-09653d3` worktree was not moved.

## B. Scope and method

Complete seven-file delta `a00a4cfbb287efb4bc60b292b1b168404a94677e..09653d32a47224a61f0e2957b3862001b869e218`, plus the affected delivery transaction, card/replay and cell-reading paths. Source review and exact-head CI only under the fleet backoff. No local builds, tests, process probes, mutation runs, boot/checks, daemon scripts or dependency operations.

## C. Finding dispositions

| Finding | Disposition | Source and proof |
|---|---|---|
| F001: failed scan allocated from zero | Closed | `crates/daemon/src/http/claims.rs:172,574-605`: floor errors propagate before the claim write; missing checkout, non-NotFound directory errors and entry errors produce `E-RS-CLAIM-SCAN` with remediation. Absent optional folders still count as empty. CI passes the adopted permission-error HTTP regression, which first verifies actual unreadability, checks the named refusal and asserts no claim was stored. The floor unit test retains absent-folder behavior and checks missing checkout refusal. |
| F002: deferred edits lost prior PRs | Closed | Delivery commits before edit generation. `store/src/claims.rs:364` reads every recorded PR for the same project/address; migration 0033 adds `deliveries_by_address`. `core/src/claims.rs:273` appends missing recorded PRs without dropping the readable cell's existing references. CI passes the adopted two-unapplied-deliveries case: `#110, #153`, then `#110, #153, #154`. Store coverage checks ordered address history and cross-project isolation; core coverage checks preservation and deduplication. |
| F003: unknown cell became empty replacement | Closed | `core/src/claims.rs:303` distinguishes known empty/text from missing section/row and incompatible content. `http/claims.rs:493-568` turns read/parse/unknown-cell failures into an explicit no-edit note, for both requirement receipts and step plan cells. The adopted unreadable-file HTTP regression passes CI and verifies the note, not just absence of an unsafe command. Missing-row/section and non-text controls pass in the core unit test. |
| N001: retry named a card never told | Closed | `http/claims.rs:378-428` uses persisted `earlier_pm` for a no-new-record replay, returns actual `pm` separately from `requested_pm`, and explains a different requested recipient without moving/resending the delivery. CI verifies the original PM/card and line remain correct and the other card stays empty. |
| F004: state-printing concern | Withdrawn | Settled by the prime: delivered prints delivered_by and the plan cell only; PRs flip their own states. No state-printing change or test is requested. |

## D. Coupled behavior

The repair preserves tagged refusals and explicit diagnostics. Failed reads no longer produce success-shaped empty data. Existing POSIX command quoting remains unchanged. The new history query is scoped by project/address with a matching index; it does not scan the spine. The original delivery/claim-release/event transaction and idempotent insert remain intact, as do the burned-number and tombstone rules. No project file write or state flip is added.

Cost: no new loop, poll, timer, sweep or launch. Claims still use in-process filesystem/store work; delivered still has the existing single gh launch per call. The added per-requirement indexed history read is request-scoped. Live deployment/cost receipts remain performance-owned.

## E. CI evidence

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42145730258

- `gh run view 42145730258 --repo AI-Substrate/pij-fleet --json headSha,status,conclusion,jobs`: exact reviewed SHA; success in Node 22, Node 24, rust-lint and rust-test.
- Rust job `127656621663` summary: **1,913 passed, five skipped**. Published `junit-linux/junit.xml` confirms all four adopted HTTP regressions passed: unreadable checkout, unreadable delivered_by, changed-PM replay and two pending receipt edits.
- The same artifact confirms the accumulated delivered_by/core cell test, absent-folder floor test, indexed address-history store test, ten-concurrent-claims test and tombstone-release test passed.
- CI's executed gate includes workspace fmt/clippy, locked metadata, doctests and nextest, plus both Node versions' typecheck/lint/tests and native contracts. No local equivalent is claimed. Log and JUnit are two views of the same CI origin, not independent runtime proof.
- Read-only git inspection confirmed the requested detached SHA and clean worktree. Initial PR inspection showed this SHA CLEAN; final inspection detected the later GitHub head noted above.

## F. Outstanding notes

There are no open correctness findings for `09653d3`. Independent local mutation/private-daemon proof was not performed under the backoff. The four formerly deferred regression candidates now have actual CI execution receipts. This is not approval of the newer `93ca2b1` head or a claim that deployment/live cost verification has happened.

## G. Handover

PM: the original a00a4cf findings are closed at `09653d3`; obtain a frozen newer-head packet and its own CI before using this verdict to land the current PR head. No reviewer build window is requested.

## H. Harness gift

**magicWand outcome:** the four focused reviewer cases are now adopted into `crates/daemon/tests/claims_http.rs` and pass CI. The earlier filesystem-error, accumulated-receipt and factual-replay lessons are executable backpressure, not just notes.

**difficulty:** the PR head changed after the requested final SHA was green. The final pin check caught it before a newer-head approval could be implied. Preserve exact-head scope and freeze/cancel queued push instructions during review; do not move a review worktree underneath its seat.
