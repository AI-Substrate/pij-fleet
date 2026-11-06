# PR #135 correctness verdict

## A. Verdict

**block**, source-pinned to `3621220d5ecac6e80b5f70ed4d36cf6b77eb8b15`, carrying unchanged to `e60be54872a25b624f86e8961b04684ec27e8b45`. All three P2 findings remain. Source/control-flow counterexamples, not locally executed reproductions.

## B. Scope

Plan 206, s15, req-28: edit, retire, merge, F19, and s05 integration. The source review was at `3621220`; the authorized final delta was reviewed in clean, pinned `~/pi-hacking/review-135-e60be54`. It changes only the private-run script/transcript and two task-state files. No product, shared-harness, skill or dependency source changed. Security and performance have separate owners.

## C. Checks

Final-head CI [42208383576](https://github.com/AI-Substrate/pij-fleet/actions/runs/42208383576) completed successfully: rust-lint, rust-test, check (22), check (24). Its Rust log reports **1,985 passed, five skipped**. Baseline CI [42206425529](https://github.com/AI-Substrate/pij-fleet/actions/runs/42206425529) also passed; its downloaded JUnit confirms execution of all five `reshape::` HTTP tests, the standing unit test and the retired/merged CLI rendering test. CI logs and JUnit are one CI origin, not independent runtime proof.

No local builds, test suites, probes, production mutations or fresh guard mutations. Hosted `ddocs-check --unavailable=warn` is not proof of local ddocs availability. No new correctness concern was found in the sequential edit/state/changelog or merge/history cases.

**Evidence delta:** the author's private run retains daemon PID 85633 and counts six launches before/after the 60-second idle interval. Edit is 42 to 51 (9); retire 51 to 60 (9); merge 60 to 71 (11); drop-queued retirement 71 to 80 (9). The refused retire, sends and view add zero in that run. Each single-document write adds seven Git and two ddocs launches; merge adds seven Git and four ddocs launches for two documents, still one commit. This matches `community/writer.rs:226-253`, `community/ddocs.rs:107-120`, and reshape's two-document batch at `community/directory.rs:667-683`.

The task-state receipt accurately describes that transcript. It is author-supplied runtime evidence, not a reviewer rerun or independent binary/source attestation. Its withdrawal requester is the actor (`told: self`), so it does not measure a different requester's FYI path. It does not exercise F001-F003 and closes none of them. Cost acceptance and public-safety scanning remain separately owned.

## D. Findings

| ID | Priority | Finding |
|---|---|---|
| F001 | P2 | Queued admission and retirement are not serialized; successful retirement can strand a concurrent request. |
| F002 | P2 | A failed post-commit withdrawal cannot be resumed through the lifecycle verbs. |
| F003 | P2 | Generated repair commands discard the project and/or role instance. |

## E. Details and required pressure

### F001: fence admission across the retirement boundary

`crates/daemon/src/community/directory.rs:617-626` reads queued requests before acquiring the writer lock. Without `--drop-queued`, nothing rechecks or removes requests admitted after that read. A valid ordering is: retire sees an empty queue; a send reads the still-active role and inserts a queued request; retire commits. Both operations succeed, but the role is retired with a queued request that assignment now refuses to drain.

The drop variant is not fully fenced either. `delivery/role_send.rs:190-191` checks standing once, then awaits assignment/liveness/store operations before insertion at `:257`. Pause that send after its active read, finish retirement and the withdrawal transaction (`community/directory.rs:698-723`), then resume insertion: the new request survives the withdrawal.

**Required:** one admission/lifecycle boundary covering the standing decision through queued insertion and the reshape preflight/commit/withdrawal. A second unprotected read is not sufficient. Add barrier-driven tests for concurrent sends with and without `--drop-queued`; no accepted request may remain queued behind a retired role.

### F002: make committed retirement cleanup resumable

`community/directory.rs:636-640,695-723` commits the retired document before withdrawing queued requests. The explicit `E-RS-STORE` path at `:715-722`, or an interruption between those phases, leaves the authoritative role retired and the queued rows intact. Retrying the same `retire --drop-queued` or `merge --drop-queued` reaches `refuse_retired` at `:640` before it can finish withdrawal. Assign and edit refuse retired roles too; directory rebuild reconstructs pointers/holdings, not this cleanup.

**Required:** a supported, idempotent continuation after the writer commit, without resurrecting the role or manually editing application state. Inject a withdrawal failure after commit, then recover with the supported operation and verify one withdrawal event/FYI and no duplicate retirement history. Also pressure the notification handoff: `http/directory.rs:826-834` records pointers and holds FYIs only after the row-deletion transaction has returned; ordinary happy-path delivery does not prove interruption recovery.

### F003: preserve the complete target in suggested actions

`community/directory.rs:641-659` reduces held rows to seat names and prints `release <role> <seat>`, losing the holding's instance. A holding of `reviewer/135` therefore produces a release for the empty instance, which cannot release that holding.

`queued_refusal` at `:1223-1255` similarly prints a bare-role assign and an unqualified retry/drop command. A queued `reviewer/135` request is not drained by an empty-instance assign: `store/role_requests.rs:201-225` selects only the assigned instance or wildcard requests. The hint must identify the actual queued instance(s).

These new actions also omit `--project`, as does F19's edit fix at `community/directory.rs:322-324`. The HTTP resolver at `http/directory.rs:1095-1128` defaults a subsequent command to the caller's repository, not the previously selected project. Calling from project A with `--project B`, then following the printed edit/release/drop command, can change A's same-named role instead of B's.

**Required:** retain the selected project and exact holding/request instance when constructing actions, including F19 and reshape refusals. Replay the emitted commands with two projects containing the same role and with named-instance holdings/queues; prove B changes or drains and A stays untouched. Existing same-project, empty-instance tests miss both boundaries.

## F. Acceptance coverage

AC1/AC2: sequential retire/merge, send refusals and preserved history execute in CI, but F001/F002 leave request lifecycle correctness incomplete. AC4/AC5: F19 and edit controls execute, with F003's scoped-action gap. AC3/AC7: s15 complete, req-28 shipped, API/skill coverage and one full-address `Delivers:` line are present. Those intended state flips are not a finding. AC6: source adds no timer/poll; each ordinary verb uses one writer commit and withdrawal FYIs are finite per request. Live cost acceptance remains performance-owned.

The SQL withdrawal itself correctly limits deletion to queued rows and appends its spine events in the same owned write transaction. That does not make the surrounding Git/store/notification sequence atomic or resumable.

**Destructive semantics:** explicit `--drop-queued` authorizes deletion. The holder/queued refusal checks are one-directional safety brakes: removing them permits more retirement. Request age is diagnostic only, not a deletion or retention policy.

## G. Evidence commands

- `git -C ~/pi-hacking/review-135-e60be54 diff --stat 3621220 e60be548`: four documentation/evidence files only; code-path diff empty.
- `git -C ~/pi-hacking/review-135-e60be54 diff --check 3621220 e60be548`: passed.
- `gh run view 42208383576 --repo AI-Substrate/pij-fleet --json headSha,status,conclusion,jobs`: final SHA, completed/success, four green jobs.
- `gh api repos/AI-Substrate/pij-fleet/actions/jobs/127862469211/logs`: 1,985 passed, five skipped.
- `gh run view 42206425529 --repo AI-Substrate/pij-fleet --json headSha,status,conclusion,jobs`: exact SHA, completed/success, four green jobs.
- `gh api repos/AI-Substrate/pij-fleet/actions/jobs/127854113718/logs`: 1,985 passed, five skipped.
- `gh run download 42206425529 --repo AI-Substrate/pij-fleet --name junit-linux --dir <session-artifacts>/pr-135-3621220-ci`: lifecycle test execution verified.
- `git -C ~/pi-hacking/review-135-3621220 status --short`: clean.

## H. Handover

Final-pin PM request `01a1b091-fc96-7860-95c7-d775bfb35856` supersedes the posting hold for request `01a1b083-eb60-70e0-a81e-d82ac88522ed`. Re-review the fix delta at a newly frozen head with exact-head CI and the focused regressions above. No product files were edited.

Both pins are verified. The [existing verdict comment](https://github.com/AI-Substrate/pij-fleet/pull/135#issuecomment-6683716514) is source-pinned to `3621220` with an explicit carry-forward to `e60be548`, as PM requested. PM appended the LOG entry as owner; verified once at `roles/reviewer/outputs/LOG.md:127`, so no duplicate was added. PM clarified that small append-only LOG writes need no baton or serialization window. Role memory was persisted through the writer in governance commit `193d26b`.

Recorded shared LOG line:

```text
2026-11-06 verdict pr-135 e60be54872a25b624f86e8961b04684ec27e8b45 block - docs-only delta leaves F001 P2 retirement/queued-admission race, F002 P2 unretryable post-commit withdrawal, F003 P2 repair-command project/instance loss unchanged. Private receipt matches writer code (edit 9, retire 9, merge 11; self-withdrawal only); exact-head CI 4/4, 1,985 passed; no local builds/tests/probes; memory via writer 193d26b.
```
