# Plan 175: prime verification of fix round 1 (pij-far-jackal, 2026-10-16)

Subject: 884386500f0339aa04757564994ad9d9adfa436d. CI run 38846430769 green: check 22, check 24, rust-lint, rust-test.

Checked against review.md (Copilot gpt-6-sol re-survey at 617fc02):
- rv-01/03/04/05: OMP/Pi linked-worktree rule stated once at peer.md:46, citing main.rs:2650 (verified in the code: `matches!(harness, Harness::Pi | Harness::Omp) && is_linked_worktree`). No remaining `spawn … --harness omp|pi` example uses a worktree cwd (grep). The orchestrator default coder is Copilot.
- rv-02: pair.md Procedure 5-6: gates run before approval; the approval report says "only after the gates pass".
- rv-06..09 (R8): bootstrap.md:48, kickoff.md:52, incidents.md:25, stream-brief.md:31 all match R8.
- rv-10: node.md:55 restricts --note to --state question|blocked.
- rv-11..13: co-38 relabelled UNVERIFIABLE and attributed.
- Scope: `git diff --name-only bb9f0b1 884386` touches only skills/pij, docs/plans/175, docs/retros, docs/how, plus the two coverage tests.
- Size: 2,503 lines, under the 2,800 ceiling.

The prime's verdict: the 7 WRONG and 3 MISSING re-survey rows are resolved. ac-0012(a) is met by the re-survey plus this verification.
