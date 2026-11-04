reviewer: pass-with-notes (head 224512db39caf3d549b50b25190e13aa6884e97a)

## A) Verdict

**F001 closed. No remaining correctness blocker or source change owed.**

The note is the deliberately dependency-free local worktree: its composite gate is not green. Focused shared-tool validation and all four exact-head CI jobs pass; this is a correctness verdict, not a claim that the full local ship gate ran successfully.

## B) Summary

This supersedes the block at `2b328dc6b71788fee466a0a8c1e0b7fc9720df1d`. Reviewed the complete repair delta in the new frozen worktree. Directory reads now return a tagged success/error result; discovery retains named errors and the CLI exits 1 before any success verdict when a root or nested directory cannot be read. The new unit regression combines a healthy nonempty root with an unreadable root.

Replayed the original chmod probe against an archive extracted from the literal full repair SHA, not ambient `HEAD`. The previously silent pass now fails with the repo-relative path and EACCES. Relative PATH entries are also skipped with a behavior test; the security and performance reviewers own their separate verdicts.

## C) Checklist

- Nine focused unit tests pass, including the mixed healthy/unreadable-root regression.
- Seventeen exact-head CLI cases pass their expected outcomes, including the original chmod failure, a missing root, an unreadable nested directory and the preserved clean/invalid/unavailable paths.
- `--unavailable=warn` cannot hide either validation errors or traversal errors when ddocs is present.
- Changed plan and backpressure documents validate with 0 errors/0 warnings; their rendered faces are current.
- Existing checks/self-check/AGENTS.md/CI wiring is unchanged from the reviewed head. C20 remains the accepted CI availability decision.
- Exact-head CI run `41768184948`: all four jobs passed.
- No source edits, installs, dependency links, commits, pushes or daemon operations were performed by this reviewer.

## D) Findings

| ID | Status | Location | Evidence |
|---|---|---|---|
| F001 | Closed | `harness/scripts/ddocs-check.ts`, directory adapter/discovery/CLI error branch | Real unreadable root now exits 1 with `docs/project: EACCES`; healthy examples cannot make it pass |

No new correctness finding.

## E) Independent repair proof

1. Clean exact-head corpus: six documents pass, exit 0.
2. Remove required `why` from the project PRD: exit 1, missing-section and render-drift diagnostics.
3. Keep that broken PRD, chmod scratch `docs/project` to 000, leaving the examples healthy: exit 1, `docs/project: EACCES: permission denied`. No success-shaped output.
4. Repeat while passing CI's `--unavailable=warn`: still exit 1 with the same path/error.
5. Restore permissions only: the still-broken document fails validation again.
6. Restore original bytes: six documents pass, exit 0.

Additional traversal controls: chmod an existing nested `docs/project/prds` directory to 000 gives a named exit-1 EACCES; temporarily remove the project root while examples remain healthy gives a named exit-1 ENOENT. Two existing but empty roots fail the zero-document guard. All scratch permissions and bytes were restored before fixture cleanup.

## F) Acceptance coverage

| Criterion | Result | Evidence |
|---|---|---|
| AC1 | Pass | Named root/nested/missing-directory failures, empty-root failure and clean/invalid/render controls |
| AC2 | Pass | Nine unit tests plus real missing-tool/bad-override controls; relative PATH test passes |
| AC3 | Carried forward, unchanged | Both local gates, inventory and CI wiring reviewed previously |
| AC4 | Pass | Fresh exact-head mutation replay, 17 asserted cases |
| AC5 | Accepted C20 ruling, not CI doc-validation proof | Missing-tool annotation remains visible; real errors still fail in warn mode |
| AC6 | Carried forward, unchanged | Planned req-27 matches C18; current corpus validates/renders clean |

Independent script and full JSON receipts: `pr113-fix-probes.mjs` and `pr113-fix-probes.json`, retained with `pr113-delta-224512d.diff` in the review session artifacts. The author's `fires.txt` cases 8-10 agree with the independent result.

## G) Validation and limitations

Commands used main-checkout binaries without installing or linking packages:

| Command | Result |
|---|---|
| `vitest run --config <main-checkout>/vitest.config.ts --root "$PWD" harness/scripts/ddocs-check.test.ts` from the repair worktree | **9 tests passed** |
| `node <session-artifacts>/pr113-fix-probes.mjs` | **17 asserted exact-head CLI cases passed** |
| `biome check harness/scripts/ddocs-check.ts harness/scripts/ddocs-check.test.ts` with shared binary | Passed, 2 files |
| `ddocs validate --json` and `ddocs build --check --json` on changed plan and backpressure | Both 0 errors/0 warnings; both faces current |
| `harness boot --json` in the new worktree | Not ready: no local node_modules, as expected from the packet |
| `harness checks --quick --json` in the new worktree | Failed six tooling stages; copilot-native passed; Rust/smoke skipped |
| Exact-head CI run `41768184948` | **All four jobs passed:** check (22), check (24), rust-lint, rust-test |

No full local green gate is claimed; dependencies were not installed or linked to make it green. No Rust build or live smoke was run by this reviewer. The shared repair worktree remained clean at the frozen SHA. Security and cost remain separately assigned lanes.

## H) Handover and harness feedback

No correctness changes owed. F001 is closed for this exact head, not merely acknowledged.

**Harness gift:** the requested mixed healthy/unreadable-root regression is now encoded in the committed test suite, and the original CLI probe independently proves the error adapter/wiring. Captured as GFT-001 in the repair review worktree.

**magicWand fulfilled:** directory-read failures can no longer become an empty-list success. Existing review-tooling limitations remain documented rather than worked around with installations.
