# PR #132 baseline security observations

Source head: 5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22.
Base/merge-base: 1035c80f1137161db070ce2a06ccaf0e8666d1eb.
Scope read: 32 files, +3370/-49, including lifecycle/authorization/writer dependencies.

**Interim only. Publication is held for the forthcoming design head and fresh worktree.** That update may already address these observations; no verdict is issued for the unreviewed replacement.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| 1 | 🟡 MEDIUM | crates/daemon/src/http/lifecycle.rs | 335, 364-450 | Memory-delayed close discards the authorization-bound target and can retire a reparented/revived replacement | 9/10 |

Ownership is checked against the initial descriptor at 335. Delayed close calls close_seat with expected None; the helper retrieves the current descriptor at 413 and supplies that fresh unchecked descriptor to tombstone_if_unchanged. The store CAS protects that new snapshot, not the identity/ownership authorized before the wait.

An authorized parent or held self-close request can therefore act after the target's parent or incarnation changes, ending current holdings and pending FYIs. Capture stable incarnation and actor context; after waiting, reject replacement/death/revival, revalidate current self/parent authority and atomically tombstone the validated descriptor. Do not compare the entire original descriptor: normal activity changes it legitimately.

## Checklist for the pending design update

- The announced incarnation check should cover both deferred paths and current ownership changes, not merely stable seat ID.
- Old memory_turn.rs:30-41,198-289 defaults to 300 seconds but accepts every positive u64 without a ceiling or checked Instant addition. Its deadline starts after awaited prompt admission and does not bound the full operation. A hard five-minute guarantee is not established by that code.
- Old held tasks are independent/volatile, and the watcher matches a prompt ID then seat idle, not captured incarnation/turn nonce. Exactly-once cancellation/restart/repeated-request behavior is not established. Reassess the new held/background design rather than assuming old observations apply unchanged.
- C21 publication candidate: docs/plans/201-role-memory/assets/evidence/private-run.txt:89 has an actual coder-checkout suffix reported outside the allowed own layout. Check/neutralize it in final evidence; literal value is not repeated here.

The unchanged memory-note path has no additional high-confidence blocker under authorized-governance authors. Req-09 does not establish holder-only writes or a non-holder flag; actor attribution is commit-only. INDEX/title/hook Markdown/controls remain, so literal-data safety and semantic prompt-injection immunity are not proved. Existing writer/S1 limits remain.

Old-head CI run 42142573572 succeeded, and its merge tree c2bfa056e290c60bf50a82b5275dafbb086e9b59 equals old head. Those tests do not establish the missing transitions/ceiling; ddocs can be unavailable with early-return cases.

Only source and existing CI evidence were used. No local builds/tests/probes/scripts, feature calls, source/governance edits, private target reads or production operations. No old-head PR comment is posted.
