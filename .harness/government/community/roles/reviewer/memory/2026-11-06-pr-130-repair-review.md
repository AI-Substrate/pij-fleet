# PR 130 repair-delta review

Final verdict: **pass-with-notes** at `8bfba9de1206a0f781bd853827df38abc961ca08`. Reviewed only the five-file 9d2a746..8bfba9d delta in fresh frozen review-130-8bfba9d. F001 remains closed; F002 closes with the recipient in the derived key and a passing two-recipient/two-attempt HTTP regression. Updated cell and overlap-warning sanitizing tests pass. Exact-head CI run 42149295542 is 4/4 green; downloaded JUnit has 1,901 tests with zero failures/errors, including both replay regressions and the existing trigger case. No local builds/tests/probes needed or run. The compact-consumption proof ceiling remains a non-blocking note under approved held-FYI semantics. Report retains the older blocked verdicts below the current final verdict.

Historical repair review follows.

Tags: reviewer, routes, compact, idempotency, recipient-scope, CI-only

Hook: a stable derived key must retain the source operation's full identity scope.

Verdict: block at `9d2a746b222f53e86383d6739db01a3ec18efa5a`, superseding dcc3e526. Report: `../outputs/2026-11/pr-130-verdict.md` (current delta verdict first, old verdict preserved below).

F001 is closed: routes-after-compact:<msg_id> is stable and the FYI row remains after claim; the adopted three-attempt same-recipient regression passes exact-head CI.

New F002 P2: the compact's identity is recipient-scoped, but the derived FYI ID omits that recipient. All generated table FYIs have local origin; the FYI PK is global (origin,id). Compact holders A and B with the same caller-chosen msg_id: both control jobs are distinct, but B's table insert collides with A's and INSERT OR IGNORE silently drops it while hold_fyi reports success. Need an unambiguous recipient component and a two-recipient CI case retaining replay coverage.

Reviewed only the seven-file dcc3e526..9d2a746 delta and dependent dedupe authorities. Run 42147270960 is exact-head green, four jobs; downloaded JUnit has 1,900 tests with zero failures/errors, including adopted replay and ordinary trigger cases. No local builds/tests/probes/mutations/harness gates; no lock acquired. New and old detached worktrees remain unchanged.

Final pin check found GitHub had advanced to 162320050313a17308e62827ed6fc3332d05cb53. The requested review remains pinned to 9d2a746; that later head is neither reviewed nor approved. Await PM authorization and a fresh worktree, never move the current reviewer checkout.

Optional control-character sanitizing stays security-owned; the earlier compact-consumption proof ceiling remains a non-blocking note.

**magicWand:** a scope-crossing key test: two recipients, same control ID, repeated before and after each claim.

**difficulties:** an exact-head green single-recipient regression does not prove a global key preserves per-recipient operations. Source and SQL identity domains must be compared explicitly.
