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
