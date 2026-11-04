reviewer: block (head 94592ad6a0ba14a97c8ae1ad2d930c4ba7185ff3)

## A. Verdict

Two correctness blockers. The original 51 targeted tests pass, but three additional HTTP regressions fail against the frozen production source. No product edits, commits, pushes, checkout movement or production-daemon operations.

## B. Findings

### F001 [P1] Same-role writes discard pending close-ends without committing them

Locations: `crates/daemon/src/community/directory.rs:408-415,433`, `:518-519,532`, and `:756-770`.

Assign and release write their target role's holders document themselves, so `flush_files` skips that role. Neither applies its pending ends to the rows it writes. Both then call `flushed` on **all** pending rows, clearing the store's retry flag even for ends absent from the commit.

Reproduced over private HTTP with real SQLite and scratch governance repos:

1. Assign `pm-001` to `pij-holder`.
2. Leave a foreign edit on its holders document and close that seat. Close succeeds, returns a durable tombstone and names `E-RS-GOV-FOREIGN-EDIT`; the holding end is pending.
3. Restore the scratch edit, then assign `pm-001` to `pij-second`.
4. Assign succeeds with `flushed: 1`. The old store row is closed with `end_pending: false`, but its committed holders row still has no `to`, `released_by` or `release_why`. The receipt even warns that the closed seat still holds the role.

The equivalent case with two holders, closing one and releasing the other, fails too. The close-end has lost its retry marker; a later rebuild restores the branch's open holding instead of the recorded end. This violates AC4 and the lineage/rebuild contract.

**Fix:** apply pending ends to the target role's rows inside the writer lock before assign/release checks and mutations. Clear only ends included in the landed document. Keep both same-role regressions, alongside the existing different-role add test.

### F002 [P2] Second-resolution timestamps collide with the holding primary key

Locations: `crates/daemon/src/community/holders.rs:46-57`, `crates/daemon/src/community/directory.rs:364,408-427`, `crates/store/migrations/0032_community_assignments.sql:23`, and `crates/store/src/community.rs:311-315`.

`from` has whole-second precision and forms part of the assignment primary key. A valid assign, release and reassign of the same seat/role/instance within one second gives two distinct holdings the same key.

Reproduced twice, in 614 ms and 278 ms. The second assign commits the new open row, then refuses `E-RS-DIRECTORY-HELD` while indexing it. Git contains the released row plus the new open row; the store contains only the released row. The suggested recovery does not repair it: `directory rebuild` returns success and `holdings: 2`, but `ON CONFLICT DO NOTHING` silently keeps only the first, closed row.

**Fix:** give every holding a durable unique identity, or allocate a collision-free `from` under serialization. Use that identity consistently for branch/store rows and end matching. Rebuild must preserve both rows, not silently discard a collision. Add a same-second release/reassign regression that checks the hook, view and rebuilt store.

## C. Coverage

| Packet contract | Evidence |
|---|---|
| AC4, normal close and absent governance | Existing HTTP test passes; close mutation detected, restored control passes |
| AC4, foreign edit | Additional tests prove close still tombstones, names the refusal and preserves the edit; subsequent same-role flushing fails, F001 |
| AC2/3/5, C1/C6 | HTTP assignment/release and hook counts pass; store transaction test covers pa mapping, unchanged roles, refusal and prime preservation; holder mapping unit tests pass |
| AC6 | Real private tmux rename test passes with counter delta exactly one; HTTP checks pane-id target and paneless receipt; control client has no diff |
| AC7/8 | Existing two-project byte-identical rebuild test passes; same-second history exposes F002 |
| AC13-15 | Adoption/foreign-edit scope, all-conflict refusal and linked-worktree HTTP tests pass |
| Schemas and docs | Embedded holders/external schemas exercised with real ddocs; pairing guard unit test passes; five fixtures have expected outcomes; required project-doc state flips present |

No new periodic path. The rename is a one-shot launch on assign, not per seat per tick. This is a correctness review, not the separate security or performance verdict.

## D. Commands and results

Cargo ran only in an immutable-head source snapshot, using the packet's shared target, `CARGO_INCREMENTAL=0`, scoped `-p`, the outer target lock and post-wait/between-command disk checks. Disk stayed at 38-39 GiB. Tests used private HTTP services and scratch git repos; the tmux test used its own server.

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --test directory_http -- --test-threads=1 --nocapture` | Original 11 pass, including real ddocs; 3 added regression tests fail |
| `cargo test --locked -p pij-daemon --lib community:: -- --test-threads=1` | 38 pass |
| `cargo test --locked -p pij-store --test store_integration assignment -- --test-threads=1` | 1 passes |
| `cargo test --locked -p pij-tmux --test adapter rename -- --test-threads=1` | 1 passes, real tmux, no skip |
| `cargo test --locked -p pij-daemon --test directory_http close_ends_holdings_and_a_refused_end_stays_pending_until_the_next_write -- --exact --nocapture` | Scratch mutation `end_pending = true` to `false`: expected failure (exit 101); restored source: passes (exit 0) |
| `cargo test --locked -p pij-daemon --test directory_http review::review_reassign_within_one_second_keeps_both_holdings -- --exact --nocapture` | F002 reproduced again; rebuild reports 2 holdings but stores 1 |
| `ddocs validate --depth 0 --json <file>` | PRD, blueprint, roadmap, plan and backpressure: 0 errors/warnings; 3 valid schema fixtures pass; 2 invalid fixtures refuse E402 as expected |
| `ddocs build --check --json <file>` | Same 5 documents: no render drift |
| `git diff --check origin/main...HEAD` | Pass |
| `harness boot --json` | Advisory NOT-READY: intentionally absent local JS dependencies |
| `harness checks --quick --json` | Not green: 6 local tooling stages fail; native Copilot contracts pass; Rust/smoke skipped; attribution warns |
| `gh pr checks 122` and exact run lookup | All 4 CI jobs pass at this exact head, run 41785782030 |

The full local done/ship gate is not claimed. No dependency installation or symlink workaround was used.

## E. Reproduction artifacts

Executed regression module: [pr-122-regression-tests.rs](pr-122-regression-tests.rs). It is a child module of the existing `crates/daemon/tests/directory_http.rs`: place it at `tests/pr122_review/mod.rs` and add `#[path = "pr122_review/mod.rs"] mod review;`.

Session receipts retained as `pr122-directory-tests.txt`, `pr122-community-tests.txt`, `pr122-store-tests.txt`, `pr122-tmux-tests.txt`, `pr122-close-mutation.txt`, `pr122-close-restored.txt`, `pr122-reassign-rebuild.txt`, `pr122-ddocs.txt` and `pr122-checks.json` under `<reviewer-session-files>`. Restored production `directory.rs` compares byte-identical to the frozen checkout.

## F. Harness feedback

**magicWand:** make pending-end flushing a next-write matrix, including same-role assign/release, and prove that every cleared pending end exists on the branch. The current single next-write case is a different-role add, so it cannot detect F001.

**difficulties:** shared-target locking and immutable scratch construction still require a hand-built wrapper. A canonical scoped review recipe should own lock lifetime, source refresh and disk-floor checks. Local JS proof is intentionally CI-only.

## G. Disposition

Block merge until F001 and F002 are fixed and reviewed at a newly frozen head. Structurally correct shipped/complete doc flips do not establish AC4/AC8 while these failures remain.

## H. Handover

Owner: pm-001. Return the two findings and regression module to the plan 195 coder. Keep security and performance verdicts independent. This verdict covers only the exact SHA above.
