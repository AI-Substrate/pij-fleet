reviewer: block (head 9919d4d9e5752ac9e5f942be268d61283daa0e04)

## A. Verdict

Two P2 correctness blockers in s12: answered requests can still be checked in/escalated from a stale round snapshot, and failed or interrupted notification attempts permanently consume their one-shot stamps.

CI is green. Findings are source/dataflow proofs, not locally executed regressions. No local builds, test runs, runtime probes or production changes.

## B. Scope

PR #133, plan 205, req-22 and AC1-AC5. Reviewed the s12 delta against stacked base 314da13a19d8d233e8c7b6d48d3de0991635b47e, including limit-column merge resolutions. #129's implementation and prior verdict are not reopened.

The packet's log command also lists the two plan/task-preparation commits, alongside feature 08b3ccb and docs 2f4be3d/7368ef0. No unrelated main changes were included as new s12 work. The detached review-133-9919d4d worktree remains clean and frozen.

## C. Checks

| Area | Result |
|---|---|
| Clock and order on the ordinary path | Later of delivery/latest card starts the first window; check-in precedes the second-window notice |
| Defaults (C5) | Existing s05 logic retained; s12's CI case proves role default, explicit override and no-limit behavior |
| Answered filtering | Initial indexed read excludes already-answered rows; F001 covers the later transition race |
| One-shot behavior | Stamps prevent repeated attempts; F002 shows they do not guarantee a notification was admitted |
| Integration/cost shape | Runs in the existing watchdog round, with one open-set read and one latest-card read per open request; no new timer or launch site |
| Documents | Narrow state/plan-cell edits and full Delivers addresses present; ddocs validation is unavailable in CI |

Security owns escalation recipient authority/audit and performance owns measured per-round scaling.

## D. Findings

| ID | Severity | Location | Concrete fix |
|---|---|---|---|
| F001 | P2 | crates/store/src/role_requests.rs:271-306; crates/daemon/src/delivery/role_send.rs:873-895 | Make notification admission conditional on the request still being unanswered/open in the same atomic transition |
| F002 | P2 | crates/daemon/src/delivery/role_send.rs:913-955,973-1023 | Persist a retryable notification claim/outbox and finalize its sent stamp only after durable admission; recover failed/interrupted attempts |

## E. Detailed findings

### F001: an answer that wins during the round is ignored

open_limited_role_requests reads unanswered requests into a snapshot. time_limits then awaits the holder's latest card before calling mark_role_request_checked_in or mark_role_request_escalated.

Both stamp methods use:

```sql
UPDATE role_requests SET <stamp> = ?2
WHERE id = ?1 AND <stamp> IS NULL
```

They do not require answered_at IS NULL. Therefore this valid schedule sends a notification after the answer has already committed:

1. The round reads an unanswered due request.
2. While its latest-card read is pending, s06 calls mark_role_request_answered and commits the answer.
3. Resume the round. The stamp UPDATE succeeds and the holder check-in or PA notice is sent anyway.

That violates the answered-skip contract of AC1/AC2 and the public answered marker used by #137. The initial SELECT is not an admission guard. Make the notification transition atomically reject a request whose answer already won, retaining one-shot and check-in-before-escalation invariants.

CI test needed: pause at the card-read seam after the open snapshot, commit answered_at, resume, and assert no check-in/notice is admitted. Cover both notification stages. Existing tests mark answers before the open query; they do not test this interleaving.

### F002: a stamp records an attempt as if the notification was sent

check_in commits checkin_at before resolving its holder and before wake_pa can audit or durably admit the message. An absent holder, failed cold-wake audit or delivery refusal leaves the stamp set. Future rounds never retry the check-in and can later accuse the holder of ignoring a message it never received.

notice is terminally worse: it commits escalated_at before reading assignments/resolving the PA/prime or attempting admission. With no live recipient, a transient assignments/audit/delivery failure, or a daemon stop between stamp and admission, no notice is sent. Yet escalated_at removes the request from role_requests_limited forever. Bringing the PA back cannot deliver the missing escalation.

This is not only the no-PA edge case: a live target plus a failed audit is enough. s05's delivering-claim boot recovery does not repair these delivered rows or their notification stamps.

Persist-before-mutate must preserve retryable work, not claim successful sending in advance. Use stable notification ids with a durable pending/claimed handoff, retry failed admission through the existing watchdog, and recover interrupted attempts. Do not fix this by simply sending before an unprotected stamp, which would introduce duplicate sends.

CI tests needed: fail check-in admission and verify retry without premature escalation; make the PA temporarily unavailable or fail notice admission and verify one eventual notice after recovery; cover interruption between persisted intent and durable message admission.

## F. Exact-head CI evidence

Run [42176950725](https://github.com/AI-Substrate/pij-fleet/actions/runs/42176950725) has head_sha 9919d4d9e5752ac9e5f942be268d61283daa0e04 and all four jobs pass: check (22), check (24), rust-lint, rust-test.

Downloaded junit-linux artifact 13484055607 and inspected actual testcase records:

| s12 testcase | Result |
|---|---|
| directory_http::a_limit_checks_in_once_then_tells_the_pa_once_and_a_card_update_spares_it | Passed |
| directory_http::a_role_default_limit_applies_and_an_explicit_limit_wins | Passed |
| store_integration::open_limited_role_requests_are_delivered_limited_and_unresolved | Passed |

The nextest log reports 1971 run, 1971 passed, 5 skipped. The named s12 cases ran. #129 restart/recovery cases also pass, supporting the merged limit-column compatibility without re-reviewing the base.

The current tests cover ordinary sequential sending, card updates before any check-in, defaults and initial answer filtering. They do not prove F001's concurrent answer or F002's unsuccessful notification paths. No mutation test or new runtime reproduction was executed by this reviewer.

## G. Commands and limits

Used the packet's commit-selection command, the stacked diff against 314da13, gh pr/run/API inspection, the downloaded JUnit XML and direct source reads. git diff --check on the s12 delta passed. No local cargo, just, harness checks or test invocation was needed or run under the CI-first instruction.

CI's ddocs step explicitly says UNAVAILABLE and that nothing was validated. State/render edits were source-reviewed; no fresh ddocs-validation claim is made.

This verdict applies only to 9919d4d. The anticipated #129 P001/index merge has not been approved by this review and needs its own pinned delta check.

## H. Handover and harness improvement

Delivery owns F001/F002. Keep AC1/AC2 and s12 completion blocked until atomic answered admission and retryable notification sending are covered by CI.

magicWand: deterministic answer-during-card-read and failed-notification/restart fixtures beside the existing successful timing case. A one-shot timestamp alone proves at-most-once attempts, not at-least-once notification admission.
