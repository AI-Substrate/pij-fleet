# PR 131 claims and delivery review

Tags: reviewer, claims, delivery, filesystem-errors, pending-edits, idempotency, C23

Hook: a printed `ddocs set` is still a replacement write; a readable old cell and all pending receipts are needed to promise append-only delivery history.

Verdict: block at `a00a4cfbb287efb4bc60b292b1b168404a94677e`, now three P2 findings. Report: `../outputs/2026-11/pr-131-verdict.md`. Four proposed HTTP regression cases: `../outputs/2026-11/pr-131-regression-tests.rs`.

F001 P2: `floor` drops directory/entry errors, falls back to zero and can allocate a number already present in an unreadable checkout. F002 P2: two delivery calls before the next docs PR emit independent snapshot replacements and erase the first PR reference when applied in order. F003 P2: unreadable/malformed document becomes an empty `delivered_by` value and yields an overwriting edit despite a warning. N001 P3: a changed-PM retry describes the new PM's card as previously told even though only the persisted original PM has the delivery.

F004 withdrawn on 2026-11-06: pm-001 relayed the prime's settled design call (5). Delivered prints delivered_by and the plan cell, not state flips; PRs flip their own states. No change is owed. The verdict, PR comment and regression candidates were corrected to match.

Core claims 5/5 and store claims 5/5 pass. Independent SQLite control loads exact migration 0033: duplicate project/kind/number rejected, other projects/kinds accepted; removing that primary key is detected. Seven ddocs validate/render clean, portability/lockfile/attribution checks pass. Exact-head CI run 42140757570 is green in all four jobs. Claim transaction, tombstone release transaction and delivery event/row commit are otherwise sound by source review.

Additional HTTP proof is deferred because of the fleet backoff; reviewers have no local build window. Initial scratch test-harness compile encountered Rust inner-doc comments through include!; that reviewer mistake was corrected in the scratch test fixture. PM subsequently confirmed that the two later shared-build waits were intentionally stopped to protect the machine under the prime's urgent backoff. This was a one-directional safety brake, not a PR defect or unexplained harness failure. No lock was cleared or another seat interrupted by this reviewer. Do not describe the proposed regressions as observed failing tests.

Local boot not-ready and quick gate has six tooling failures, native Copilot passes. C25 respected: no install or dependency symlink. No product edits, commits, pushes, live-daemon operations or deployment.

The local receipts above were collected before this seat received the backoff instruction. Since receiving it: source review and CI evidence only, with no local builds, test runs, probes or scripts until the prime lifts it. The blocking verdict does not wait on deferred HTTP proof.

magicWand: encode the unreadable-input, two-pending-receipt and changed-recipient matrix into claims_http; this review supplies bounded cases instead of only prose.

difficulties: quick checks execute incompatible ambient tools after boot has established missing local dependencies. The build cancellations were intentional backoff enforcement; preserve the reason in a durable receipt and prefer explicit CI-only review-readiness, never a lock bypass.

## Repair review at 09653d3

Pass-with-notes at `09653d32a47224a61f0e2957b3862001b869e218`. F001-F003 and N001 closed; F004 remains withdrawn. Report: `../outputs/2026-11/pr-131-fix-verdict.md`.

The complete seven-file repair delta propagates E-RS-CLAIM-SCAN before writes, accumulates persisted address deliveries through a new index after recording, distinguishes unknown cells and prints no unsafe replacement, and reports the persisted card recipient separately from requested_pm. Four reviewer regressions are adopted and actually pass in exact-head CI run 42145730258, with all four jobs green and 1,913 nextest tests passed/five skipped. Source plus CI only; no local builds/tests/probes/mutations under backoff.

Final pin check found GitHub had advanced to `93ca2b1c3dc80d3ea609438e9864d9593b7e35f5`, BLOCKED. This pass covers only requested 09653d3, not the newer head. PM notified; detached review worktree unchanged. No outstanding correctness changes at 09653d3.

magicWand outcome: the prior cases are now deterministic CI backpressure. Final-head drift remains a coordination hazard; keep the exact-head qualification prominent and do not silently repin.

## Final card-path review at 93ca2b1

Pass-with-notes at `93ca2b1c3dc80d3ea609438e9864d9593b7e35f5`, after PM explicitly authorized the re-pin and late performance ask. Report: `../outputs/2026-11/pr-131-final-verdict.md`.

Only the five-file 09653d3..93ca2b1 delta was re-reviewed. Delivery reads move out of shared seat_card into singular /v1/state, so bulk /v1/cards leaves deliveries empty and does no history query. The singular read walks deliveries_by_pm newest-first with LIMIT 48, groups at most three PRs in Rust, and never sorts/groups the whole history in SQL. A new EXPLAIN CI control checks index use and no TEMP B-TREE.

Exact final-head CI run 42146651607 is all green after waiting: 1,914 nextest passed/five skipped. JUnit confirms the new index control, state-card wire/card delivery controls and all four adopted regressions. F001-F003 and N001 remain closed; F004 remains withdrawn. Final GitHub/worktree SHA matches and PR is CLEAN. No local builds/tests/probes/scripts or product changes under backoff.

Nonblocking qualification: the 48-row window can hold fewer than three PRs or a partial address list for a large delivery. This is a bounded card summary, not a complete ledger; backing records are intact. Prefer explicit summary/truncation wording and a 48/49-row fixture, not an unbounded read.

## Covering-index repair at c6aa07b

Pass-with-notes at `c6aa07bfeb7316ee277f7325f873c3684e8eb4fa`. Only the four-file 93ca2b1..c6aa07b delta and affected store/card paths reviewed. Report: `../outputs/2026-11/pr-131-c6aa07b-verdict.md`.

The 48-row completeness qualification is closed: a covering pm/time/project/pr index selects the newest three distinct PRs by timestamp stepping; a PK read retrieves every address for each selected PR. One read transaction keeps the phases consistent. Production query-plan guards cover both reads with no TEMP B-TREE; the new 60-address fixture proves complete newest PR plus both older PRs.

Exact-head CI run 42151520958: all four jobs pass, 1,915 nextest passed/five skipped. JUnit confirms both new index/completeness controls and all four original HTTP regressions. Original F001-F003/N001 remain closed, F004 withdrawn. No local checks needed/run under the current CI-first policy; worktree/GitHub pin matches and PR is CLEAN.

N002 nonblocking: after a later invocation adds addresses, the aggregate card uses newest recorded_at but recorder metadata from the lexicographically first address, which can be an earlier recorder. Follow up by aligning metadata to the selected timestamp or documenting aggregate provenance; source qualification only, no local reproduction. No records/addresses lost.

magicWand outcome: large-delivery completeness is now executable CI proof; extend the fixture family to ties and later-address/different-recorder cases.
