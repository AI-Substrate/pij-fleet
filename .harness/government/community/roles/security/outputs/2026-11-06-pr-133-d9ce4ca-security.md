# PR #133 pinned-notice security verdict

security: pass-with-notes (head d9ce4ca99b020cf2ef11f7a515068076dcbd0392)

No security vulnerabilities found in the complete seven-file 44f1eea..d9ce4ca delta. Prior s12 qualifications otherwise stand.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| - | - | - | - | No security vulnerabilities found in the pin/entitlement delta | - |

## A pin is evidence context, not dispatch authority

At [role_send.rs:942-1004](https://github.com/AI-Substrate/pij-fleet/blob/d9ce4ca99b020cf2ef11f7a515068076dcbd0392/crates/daemon/src/delivery/role_send.rs#L942-L1004), positive admission evidence for the old pin only settles the stamp and returns an action: it never invokes delivery or wakes that old seat. With no evidence, processing follows the normal fresh check-in/notice resolution path.

The returned action is an outcome, not a queued command: the watchdog maps it into RequestLimit, and its production runner discards successful round results rather than dispatching the recorded target.

[Admission probes:154-162](https://github.com/AI-Substrate/pij-fleet/blob/d9ce4ca99b020cf2ef11f7a515068076dcbd0392/crates/store/src/role_requests.rs#L154-L162) bind the pinned recipient and fixed request/step message ID separately and require local origin across ledger, job or FYI evidence. Probe errors are logged and retain the pin for another leased retry, not used as permission to send elsewhere.

New sends [pin the newly resolved descriptor:1074-1110](https://github.com/AI-Substrate/pij-fleet/blob/d9ce4ca99b020cf2ef11f7a515068076dcbd0392/crates/daemon/src/delivery/role_send.rs#L1074-L1110); a lost claim sends nothing. [Pin/settlement methods:376-461](https://github.com/AI-Substrate/pij-fleet/blob/d9ce4ca99b020cf2ef11f7a515068076dcbd0392/crates/store/src/role_requests.rs#L376-L461) qualify updates by request and claim timestamp. Release and finish clear the target.

Fresh notices still read assignments for the request project and select its live PA, then prime fallbacks. Metadata-only bodies, original-delivery-holder check-ins and mandatory daemon-attributed cold audit before dispatch remain. Previously admitted work is not revoked by PA rotation. The pin adds no atomic lock spanning assignment resolution and delivery; prior resolution/admission race qualifications remain.

## CI, rerun and limits

Clean detached worktree matches GitHub's full head; PR is OPEN/MERGEABLE/CLEAN. [Run 42209529237](https://github.com/AI-Substrate/pij-fleet/actions/runs/42209529237) attempt 2 has all four checks SUCCESS. JUnit artifact 13498921432 records 1,989 passing cases, zero failures/errors; archive SHA-256 verified. Both rotated-PA regressions, store pin/lease semantics and the indexed admission-probe guard passed.

Attempt 1 artifact 13497064652 has one recorded failure, separate_adapters_cannot_both_reserve_one_pane in pij-tmux; that case passed on attempt 2. The pin-specific cases passed on both attempts. Neither an unqualified first-attempt success nor a new local rerun is claimed.

Rust CI checked out synthetic merge b51b84ac8fcf0778eb2f084df94cc5f77cc4acd4, tree 6a2db568e7f013329afede3801db128001c3cd1d. Head tree is ede91e92d3a44dd9b69ea034f6942aadc2f87a40. Exact comparison lists only CLI/OMP/shim/docs differences; s12 source/tests and Cargo inputs are identical. This is scoped implementation/test proof, not whole-tree equality.

Retained admission records support deduplication, not unconditional exactly-once execution or recipient ACK. Two-minute leases assume a progressing wall clock; timestamp ownership is not a unique nonce. Answer-after-claim suppression and live-holder admission-failure injection remain unproved.

All 411 added lines passed calibrated path/credential/foreign-GitHub checks; the complete delta was manually read. Pattern scans are not universal secret detectors. Publication text was separately screened. No local builds, tests, process probes, reviewed-feature calls or source mutations were performed.
