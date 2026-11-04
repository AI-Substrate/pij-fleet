reviewer: pass-with-notes (head 4ec59a851d224b0a7736c6366ee9f05684f8b447)

F001 and F002 are closed. No new blocking correctness finding in the scoped repair or main-merge review. This supersedes the block at b28d815; unchanged baseline evidence is carried forward, not claimed as rerun. Security's independent verdict and disposition of the documented lstat/write race are still required; this is not a security or measured-cost approval.

Final-head delta: read the complete `git diff a4bb4fa 4ec59a8`, exactly one added D-190-4 row in `docs/difficulties.md`. It records the already reported test flake and rerun receipts, without changing code, tests, schemas or dependencies. No new finding. The a4bb4fa runtime and mutation evidence below carries forward unchanged; no rebuild or checkout movement.

## Repair findings

| Item | Disposition | Observed proof |
|---|---|---|
| F001: missing brief commits a batch | Closed | The writer checks brief targets before creating its undo snapshot or writing any file. Missing, absolute, parent-traversing, outside-symlink and directory targets refuse `E-RS-ROLE-BRIEF-MISSING`, naming the role and brief. Existing readable regular-file control passes. The real-ddocs missing-brief regression and validator-independent batch test both pass with HEAD/index/status unchanged. |
| F002: relative PATH breaks ddocs after cwd changes | Closed | PATH entries are resolved against the daemon-start cwd and retained only as absolute paths. Actual launches from a different governance cwd pass for `tools`, `./tools`, empty and interior-empty PATH components; absolute override precedence remains covered. |
| S1: pre-existing symlink destinations escape the root | Static reproductions prevented | Dangling tracked destination links, linked ancestors, rendered siblings, schema-copy destinations and `.dd` ancestors refuse `E-RS-GOV-SYMLINK` before any write. The tests preserve HEAD/status/the link and assert no external output. Original checkout safety, batch atomicity, lock retry and commit-failure rollback controls still pass. |

**Remaining note:** confinement is lstat-based, not an atomic no-follow/openat guarantee. An unrelated local process can swap an entry between inspection and write, including around renderer or rollback operations. The per-worktree mutex serializes this daemon's writers, not external filesystem actors. The packet and implementation evidence already identify this residual; security owns its risk grade and merge disposition. No waiver is implied here.

The symlink veto is a **one-directional safety interlock (a brake), not a policy**: removing it permits more writes. It does not choose a different retention or ownership outcome.

## Mutation evidence

In an immutable-a4bb4fa scratch extraction, bypassing the brief check, restoring the old PATH lookup, and disabling the symlink veto produces exactly six expected failures: two F001 tests, the F002 execution-level test, and three S1 tests. The other 26 tests pass. Restoring the source bytes makes all 32 pass again; the restored writer and ddocs files were compared byte-for-byte with the frozen checkout before that rerun. Neither review worktree was edited.

## Merge review

The two merges preserve both req-26 and req-27, retain req-03's C8 wording/state while taking main's corrected source address, and retain both roadmap s14 and s02 history. The merged PRD and roadmap validate and their generated Markdown has no drift. Main's harness/schema-sensor changes are inherited, not reimplemented by this PR.

`git merge-tree --write-tree a4bb4fa1e9786667051897156e78c3143a75303e 4b14b21` succeeds without conflicts. `Cargo.lock` and `crates/daemon/Cargo.toml` have no differences against main 4b14b21. The branch includes main through 795c9b7; this clean-merge proof does not claim that later 4b14b21 is already an ancestor.

## Commands and results

All cargo commands used `CARGO_TARGET_DIR=<coder-target>` and `CARGO_INCREMENTAL=0`, with package scope. Disk was checked before builds and stayed above the 25 GiB stop floor, from 30 GiB initially to 26-27 GiB near closeout. No dependency installs, product edits, commits, pushes, deployment or production-daemon testing.

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --lib community -- --nocapture` | PASS: 32 tests on the frozen repair checkout; real ddocs ran |
| `cargo test --locked -p pij-daemon --test directory_http -- --nocapture` | PASS: 3 tests |
| `cargo test --locked -p pij-testkit --test spawn_sites -- --nocapture` | PASS: 4 tests |
| Scratch `cargo test --locked --manifest-path <scratch>/Cargo.toml -p pij-daemon --lib community -- --nocapture` with the three guards weakened | EXPECTED FAIL: 26 pass, 6 fail |
| Same scratch command after exact source restoration | PASS: all 32 |
| `cargo fmt -p pij-daemon -- --check` | PASS |
| `cargo clippy --locked -p pij-daemon --all-targets -- -D warnings` | PASS |
| `ddocs --json validate <document> --depth 0` for PRD 001, roadmap 001 and phase-2 tasks | PASS: all 3, zero errors/warnings |
| `ddocs --json build <document> --check` for those same documents | PASS: all 3, no drift |
| `git diff --check b28d815 a4bb4fa` | PASS |
| `git diff --check a4bb4fa 4ec59a8` | PASS: complete one-line documentation delta read |
| Merge-tree against main 4b14b21; dependency-manifest comparison | PASS: no conflicts; both named manifests identical |
| `harness boot --json` | Advisory NOT-READY: intentionally absent linked-worktree JS dependencies |
| `harness checks --quick --json` | FAIL: 6 JS/tooling stages; Copilot-native PASS; attribution WARN; Rust and smoke explicitly skipped |

The full local composite gate is not claimed green. Exact-a4bb4fa CI passes all four jobs, [run 41773746384](https://github.com/AI-Substrate/pij-fleet/actions/runs/41773746384); the final 4ec59a8 head also passes all four jobs, [run 41774675967](https://github.com/AI-Substrate/pij-fleet/actions/runs/41774675967). There are no added launch sites or timer/per-seat loops in the repair; spawn-site contracts pass. Performance owns the separate cost verdict.

The new review checkout remains clean at a4bb4fa; the original checkout remains at d5fd8cf. GitHub's head matches this final verdict: `4ec59a851d224b0a7736c6366ee9f05684f8b447`.

## Handoff and harness feedback

No correctness fix is owed by this scoped review. Obtain the independent security verdict for the residual before merging, and preserve exact-head attribution if the PR moves again.

Evidence: `<reviewer-session-files>/pr114-fix-delta.diff`, `pr114-fix-mutation-results.txt` and `pr114-fix-restored-results.txt`. Machine-local build/session paths are deliberately represented by placeholders.

**magicWand fulfilled:** the real-ddocs warning-bearing negative case, readable-brief control and cross-cwd executable control now live in the committed regression suite, rather than only in a review transcript.

**difficulties:** local composite proof remains tooling-limited; a shared Cargo target contended briefly during review. No workaround installs or competing target were introduced. The new worktree's ignored observation buffer records the proof boundary as INS-001.
