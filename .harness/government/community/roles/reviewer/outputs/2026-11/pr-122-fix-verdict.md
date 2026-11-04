reviewer: pass-with-notes (head e4527770a69db16cf5b3a85df528d4e08c21672a)

## Verdict

F001 and F002 are closed. No remaining product-correctness blocker found in the complete eight-file repair delta from 94592ad. This supersedes the earlier block for those findings. One non-blocking test-clock note remains below.

Reviewed in the new frozen `fleet-review-122b` worktree. The original checkout stays at 94592ad. Both are clean; neither HEAD moved.

## Repairs verified

**F001:** assign and release apply their target role's pending ends before checking/mutating its holders, inside the writer lock. The build returns the actual landed ends, and all writer paths clear only those ends. Both same-role HTTP regressions now pass. An extra scratch HTTP control removes the old holding from the branch: a subsequent successful same-role assign reports `flushed: 0` and leaves the missing holding's end pending.

**F002:** millisecond timestamps plus collision-aware `fresh_from` allocation under the writer lock give repeated holdings distinct keys. The HTTP regression preserves both rows through rebuild. Duplicate-holder validation runs before replacing indexes; an extra scratch HTTP control commits a duplicate row, receives `E-RS-DIRECTORY-ROW`, and verifies both assignment rows and document pointers are unchanged. The store's own duplicate-rebuild test verifies transactional rollback rather than silently retaining the first row.

## F003 [P3, non-blocking] The same-second regression has a wall-clock boundary flake

Location: `crates/daemon/tests/pr122_review/mod.rs:104-106,120-124`.

The test waits for `holders::now()` to change. That previously aligned to a second, but the repair makes it change every millisecond. Its later assertion that both assignments share a wall-clock second can therefore fail despite correct, distinct holdings.

Observed with byte-identical restored production source and two additional scratch boundary tests: the sequence took 440 ms, both assigns succeeded, but the first `from` was `06:59:21.819Z` and the second `06:59:22.079Z`. The only failure was `the same-second precondition must hold`. The exact-head 14-test run and eight subsequent targeted repeats passed; that does not remove the boundary race.

**Fix:** guarantee the scenario with an injected clock, or keep collision pressure in a deterministic allocator test and remove the wall-second assertion from the HTTP lifecycle/rebuild test. At minimum, do not use a millisecond change as evidence of a second boundary. This is a test-only false failure, not a recurrence of F002.

## Evidence

Every Cargo invocation held the packet's shared-target lock, used `CARGO_INCREMENTAL=0` and scoped `-p`, refreshed workspace source timestamps after acquiring the lock, and checked free disk after waiting and between commands. Disk stayed at 34-36 GiB. HTTP services, git repos and mutation sources were private.

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --test directory_http -- --test-threads=1 --nocapture` in the exact-head checkout | 14 pass, including all three original review regressions and real ddocs |
| `cargo test --locked -p pij-daemon --lib community:: -- --test-threads=1` | 41 pass |
| `cargo test --locked -p pij-store --test store_integration assignment -- --test-threads=1` | 1 passes, including duplicate-rebuild rollback |
| `cargo test --locked -p pij-daemon --test directory_http review::review_assign_same_role_flushes_a_foreign_edit_blocked_close -- --exact --nocapture` with the F001 repair disabled in scratch | Expected failure, exit 101 |
| `cargo test --locked -p pij-daemon --lib community::holders::tests::a_fresh_from_never_repeats_the_same_seats_holding -- --exact --nocapture` with collision checking bypassed in scratch | Expected failure, exit 101; only 2 unique values instead of 50 |
| Same HTTP suite after restoring production source, with two extra boundary controls | 15 pass, 1 test-clock precondition failure described in F003; both new boundary controls pass |
| `cargo test --locked -p pij-daemon --lib community::holders::tests:: -- --test-threads=1` after restore | 8 pass |
| Targeted same-second HTTP test after restore, eight repeats | 8 pass; not claimed as a fix for F003 |
| `git diff --check 94592ad..HEAD` | Pass |
| `harness checks --quick --json` in the repair checkout | Same six local tooling failures; native Copilot passes; Rust/smoke skipped; attribution warns |
| `gh pr checks 122 --watch --interval 20` plus exact run lookup | All four jobs pass at e452777, run 41788564917 |

No full local gate is claimed. No dependency installation/link, product edit, commit, push or live-daemon operation. Complete restored crate-tree comparison differs only in the scratch boundary-test module.

## Artifacts and handover

Additional executed boundary controls: [pr-122-fix-boundary-tests.rs](pr-122-fix-boundary-tests.rs). Receipts retained under `<reviewer-session-files>` as `pr122b-http.txt`, `pr122b-community.txt`, `pr122b-store.txt`, `pr122b-f001-mutation.txt`, `pr122b-f002-mutation.txt`, `pr122b-restored-http.txt`, `pr122b-restored-holders.txt`, `pr122b-clock-repeat-1.txt` through `-8.txt`, and `pr122b-checks.json`.

**magicWand:** make the timestamp-collision proof clock-controlled so fixture setup and scheduler timing cannot determine the verdict. The committed same-role regressions now encode the original pending-end lesson.

The repair adds no process-launch site or periodic path. Security and performance verdicts remain independent. Owner: pm-001. Any further edit needs its own frozen head.
