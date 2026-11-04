# PR 122 assignment and lineage review

Tags: reviewer, directory, close, pending-ends, lineage, mutation, shared-target

Hook: when a write clears a pending-end flag, assert the committed branch actually contains the end; a receipt count is not proof.

Verdict: block at frozen `94592ad6a0ba14a97c8ae1ad2d930c4ba7185ff3`. Report: `../outputs/2026-11/pr-122-verdict.md`. Executed extra HTTP tests: `../outputs/2026-11/pr-122-regression-tests.rs`.

F001 (P1): assign/release skip their own role in flush_files without applying pending ends to the holders rows they do write, then clear every pending flag. Both same-role regressions fail: close succeeds through a foreign edit and records a pending end; the next same-role operation reports flushed:1 while leaving that end absent from the branch. Store and branch diverge, and the pending retry marker is gone.

F002 (P2): holders::now truncates to seconds, while the holding PK includes from_at. Assign/release/reassign of the same role/instance/seat in one second commits a second holding then refuses indexing. Reproduced in 614 ms and 278 ms. Rebuild returns holdings:2 but its conflict-ignore insert stores only the first closed row.

The original 11 directory HTTP tests (including real ddocs), 38 community unit tests, 1 store assignment test and 1 real private-tmux rename test pass. Scratch mutation changes close's end_pending=true to false; the existing close test fails, restored source passes. Production directory.rs then compares byte-identical to frozen source. Cargo was always scoped, incremental disabled, under the shared-target lock, with disk checks after waiting and between commands; 38-39 GiB free.

Five DD documents validate 0/0 and render without drift. Schema fixtures give 3 expected passes and 2 E402 failures. Exact-head CI run 41785782030 passes all four jobs. Boot remains not-ready; quick checks fail six local tooling stages, native Copilot passes. No install or dependency-link workaround, full local gate claim, product edit, commit, push or live-daemon operation.

magicWand: encode a next-write matrix for pending close-ends, especially same-role assign/release, and assert branch content before allowing the retry marker to clear.

difficulties: constructing immutable snapshots, outer lock ownership and post-wait disk-floor checks remain a hand-written wrapper. Add a scoped engineering-harness review command rather than requiring every reviewer to reconstruct it. These observations were also captured through harness observe; read-only review leaves encoding to the owner.

## Repair review at e452777

Pass-with-notes at `e4527770a69db16cf5b3a85df528d4e08c21672a`, in new frozen fleet-review-122b. Full eight-file one-commit delta reviewed. Report: `../outputs/2026-11/pr-122-fix-verdict.md`.

F001/F002 closed: pending ends apply under the writer lock and only landed ends clear; from allocation is unique under that lock; duplicate rows refuse before index replacement and store replacement rolls back. All 14 HTTP, 41 community and 1 store tests pass. Both repair mutations fail expected controls. Restored source matches the frozen crate tree except the added scratch tests. Missing-branch holding remains pending with flushed:0; duplicate branch rebuild refuses and preserves both assignments and pointers.

F003 P3 is test-only: waiting for holders::now to change now waits for a millisecond, not a second. The unchanged same-second HTTP test falsely failed in a restored-source scratch run after 440 ms, with from times 06:59:21.819Z and 06:59:22.079Z; both assigns succeeded and held distinct keys. Eight subsequent targeted repeats pass, not a repair. Recommend clock injection or remove the wall-second requirement from the lifecycle test and keep collision pressure deterministic at the allocator.

CI run 41788564917 is all green at the exact repair head. Local quick gate remains six tooling failures, native Copilot passes. Cargo locked/scoped throughout, incremental disabled, post-wait and between-command disk checks at 34-36 GiB. No product edits or commits; old and new worktree HEADs preserved.

magicWand: a controlled-clock fixture for collision tests. The original same-role regression lesson is now encoded in the product suite.
