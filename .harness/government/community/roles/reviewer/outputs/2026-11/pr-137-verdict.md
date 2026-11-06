# PR #137 s06 review

## Verdict

**block**, exact head `4ab6460fc70d4d0cf9dda07910d9664fb4f8546a`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/137#issuecomment-6680508651

Two P2 identity-scope findings block approval. N001 is a nonblocking diagnostics note. Exact-head CI is green; the supplied cases do not cover the failing identity boundaries.

This is the requested early review of s06's own commits over `72f6d98021a7e425cd40354b6b6d7af7ca60520a`, excluding main and plan/205's own work. The draft is currently DIRTY against its moving stacked base. It still needs the planned main merge/retarget and a fresh exact-head delta review; this verdict is not a merge-result assessment.

## Findings

| Id | Severity | Location | Finding |
|---|---|---|---|
| F001 | P2 | `crates/store/migrations/0036_community_outputs.sql:7-20`; `crates/store/src/outputs.rs:60-80`; `community/output.rs:158-182` | Relative output paths are unique only inside one project's governance worktree, but the store key is globally path-only. A second project's output is silently absent from the index. |
| F002 | P2 | `crates/daemon/src/http/output.rs:326-342` | Notification identity is writer plus wall-clock milliseconds. Distinct outputs can share it and be deduplicated as one notification. |
| N001 | P3 | `crates/daemon/src/http/output.rs:347-405` | Notification lookup/guard errors lose their actual cause, making post-commit failures look like a refused wake or no seat above. |

### F001: namespace the store identity by project

Two independent projects may both write the first `note` for subject `status` on the same day under role reviewer. Each writer correctly selects:

`.harness/government/community/roles/reviewer/outputs/2026-11/06-note-status.dd.json`

The roots are different, so both files and commits are legitimate. The stored path is relative, however, and `community_outputs.path` alone is the primary key. `record_output` uses `ON CONFLICT(path) DO NOTHING` and ignores the zero affected rows. The second project therefore receives a successful write/notification response while `outputs_for_role` and `outputs_for_subject` cannot find its output. The first project's row remains, so a query for a previously empty second project is not a useful cross-project control.

**Observed storage-boundary proof:** an isolated SQLite oracle loaded the exact migration and production insert shape. Alpha's first insert succeeded; same-project replay was correctly idempotent; beta's identical relative path was silently ignored and beta's read returned zero. Giving beta a different path succeeded, confirming that the instrument can index/query beta.

**Fix:** use `(project, path)` as the output identity and conflict target, or an equivalently project-qualified durable key. The adjacent community-pointer store already uses this composite identity. Preserve idempotence within a project, not across unrelated governance roots. Add a store/HTTP regression that actually inserts the same relative path into two projects and reads both back.

### F002: use committed output identity, not a timestamp, for the notification

`format!("output-{}-{}", writer.id, now_ms())` has no uniqueness guarantee. The supported concurrent-output flow allows one request to pause in its post-commit indexing while another completes; both notification continuations can execute in the same millisecond. Different subjects, output paths, request IDs and recipients are not part of this key.

This key is not decorative. `DeliveryService::hold_fyi` copies it into `HeldFyi.id`; `SqliteQueue::hold_fyi` performs `INSERT OR IGNORE`, and the authoritative FYI key is `(origin, id)` (migrations 0023/0027). The second distinct FYI is then dropped while both callers receive `held`. Ordinary wake delivery also uses msg_id as its dedupe/correlation identity. The current concurrent HTTP test checks distinct files and clean git status, but never checks that both notifications survive.

**Proof grade:** source establishes the colliding ID construction and dedupe path. The niced SQLite boundary oracle supplied two distinct output notices with the equal-clock IDs the constructor permits: only the first recipient/body survived. A distinct-ID positive control retained the second. This is not a claimed end-to-end HTTP scheduling reproduction.

**Fix:** use the existing unique message allocator or derive the notification ID from the committed output's immutable identity, e.g. its commit plus project/path. Do not repair this with sleeps or polling. Add a deterministic equal-timestamp/concurrent-output case that verifies two notification records, not just two output files, while a genuine replay of one message remains idempotent.

### N001: preserve post-commit notification failure diagnostics

`cold_wake::guard` failures are discarded as `"the forced wake was refused"`; `above` converts registry/store failures to absence with `.ok()`. The writer cannot distinguish an unavailable lookup/audit from a real lack of an upper seat or recover using the original error.

Keep the output's committed path/commit in the result, but return structured notification failure with its underlying cause, or use the repository's explicit partial-failure pattern. Do not send after an audit failure. This is a follow-up, not an additional merge blocker.

## What otherwise checks out

- The writer chooses filenames under its existing lock, writes one pij/output document plus rendering, records writer/holding status and retains existing files. Distinct same-project output filenames are covered by CI.
- Verdicts require a head/result; subject reads use exact `head = ?` equality, never a prefix/latest-head fallback. The wrong-head control returns no output.
- Requested mode selection correctly implements wake/fyi/none; block/failed results override to wake. The wake path calls the existing forced/audited cold guard before delivery. A handover requires an in-flight body.
- The default unrequested path chooses the live recorded parent, else the current prime, and sends to one seat; the ordinary unrequested FYI case passes CI.
- `mark_role_request_answered` is called after the output fact is recorded. The imported #133 helper sets answered_at and s12 skips it. Its implementation is treated as the declared dependency, not re-reviewed as s06 work.
- CLI/shim endpoints, body-file forwarding, help, role-output API and schema/store wiring are present. The project-state changes match s06's stated rows; accepted-state bookkeeping cannot close F001/F002.

Cold-output integration itself is not demonstrated by the four new HTTP tests: they use warm fixtures. The forced-audit path is checked by source through the existing guard, not represented as a live cold-requester witness. Security's role/authority/link verdict remains separately owned.

## Evidence and cost

CI run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42155525133

Exact head, all four jobs successful: Node 22, Node 24, rust-lint and rust-test. Rust nextest summary: **1,916 passed, five skipped**. Downloaded JUnit confirms all four output HTTP tests, exact-head/store listing, required verdict fields, block/failed predicate and CLI output wiring controls.

The one additional local check addressed CI gaps in cross-project and notification identity. It ran as:

`nice -n 19 taskpolicy -b python <oracle> <frozen-review-root>`

The source is archived alongside this report as `pr-137-identity-oracle.py` (executed from the session-file copy before archival). It uses SQLite in memory and the exact migrations, no daemon, transport, cargo target or Rust build. Both failing boundaries and positive controls reproduced. No full local suite, actual cold requester or live fleet run is claimed.

The committed private-daemon evidence reports one writer commit per output, git +7/ddocs +2 and no notification launches; reads launch nothing. No new timer/poll was found in s06. The notification-ID repair should preserve that shape. Performance owns live cost grading/AFTER.

## Handover and harness gift

Close F001/F002 with scoped regression cases, then supply the frozen repair SHA. After #129/#133 land, separately review the merge/retarget delta and its CI. No request is made to move this detached worktree.

**magicWand:** test the namespace that owns each key. A governance-relative path needs its project, and a delivery dedupe ID needs committed message/output identity rather than a reading of the wall clock. The current happy-path project filter and two-file concurrency controls cannot prove those properties.

**difficulties:** the initial plan glob missed a tracked file; the scoped git manifest identified `200-outputs-notify`. The new identity-boundary oracle filled a specific CI gap at low QoS without consuming a Rust build slot.

Role memory was written through the live verb using a real body file, at writer commit `c56475d083225859fc270358777cdf34972df9f2`, note `2026-11-06-pr-137-output-identity-review-4ab6460.dd.json`. No role note or generated INDEX block was hand-edited.
