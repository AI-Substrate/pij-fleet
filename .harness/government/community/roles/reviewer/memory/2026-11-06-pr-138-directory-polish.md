# PR 138 directory-polish review

Tags: reviewer, batch-assign, lineage, clock-injection, shared-build, CI-first

Hook: separate writer atomicity from post-commit indexing, and exhaustive diagnostics from phase-local validation.

Verdict: pass-with-notes at `7c2b6b147d158746545699226343a3fd09b7bdeb`. Report: `../outputs/2026-11/pr-138-verdict.md`. No correctness blocker; N001 P3 qualifies “every bad line” wording.

Final pin remains 7c2b6b1, but GitHub changed from CLEAN to DIRTY against current main during review. Existing head CI stays green; this is not a merge-ready or conflict-resolution approval. PM owns reconciliation and a new head needs appropriate fresh CI/review. The detached reviewer checkout was not moved.

Batch assign validates before one writer commit; the service builds all role rows under its lock, preserves request order and reuses the single-assign side effects. Lineage import is prime-gated, handles closed/unregistered seats, checks overlaps under the writer lock and sets no seat role. UTC stamps/local view, first-open-holding rename rule, marker/retry and shared-build seat/nice/taskpolicy changes are present.

N001: HTTP preflight returns on malformed/invalid-seat lines before role existence/already-held checks in the service. Existing bad-batch fixture includes missing qa but asserts only unknown-seat/malformed diagnostics. No partial write, but not every mixed failure appears in one response.

F003's fixed-clock collision unit test and wall-clock-independent HTTP reassign/rebuild test pass exact-head CI. Coder's reported 50/50 local runs/private receipts are from predecessor 6e40ada, not claimed as reviewer final-head measurements.

Run 42156546796 is exact-head 4/4 green; JUnit 1,894 tests zero failures/errors; Node log shows all 15 shared-build tests pass. No local test/build/mutation/probe, no lock acquired, no product changes/commits/pushes. Read-only filesystem inspection confirms the actual Mac nice/taskpolicy paths exist; real QoS and live AFTER receipts stay performance-owned. ddocs CI is UNAVAILABLE.

Security owns filesystem/input/wrapper threat assessment. Marker is a best-effort hint, not lock authority. Post-commit index errors name the committed SHA; the batch guarantee is the authoritative git commit, not a distributed git/SQLite transaction.

**magicWand fulfilled:** deterministic injected clock and encoded per-command scheduling remove repeated timing and prefix inference from the harness.

**difficulties:** phase-local diagnostics can look exhaustive in a green happy-path test; fake taskpolicy flag propagation is not a measurement of actual scheduling state.
