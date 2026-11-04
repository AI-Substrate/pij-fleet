# PR 121 fix-head correctness review

**Superseded:** [final verdict](pr-121-final-verdict.md) passes with notes at `2ecbd26a8437593f03e27b1b7e412862d7900baf`; F001 and F002 are closed. This file records the earlier repair-head review.

**reviewer: block (head 30b3a371067450e2275d3e3926736a8479a70d0c)**

Supersedes the original e13a834 verdict. F001 is closed. F002's existing-symlink case is closed, but target-identity exclusion still fails for an initially missing target ending in `/`.

## Findings

| ID | Priority | Location | Disposition |
|---|---|---|---|
| F001 | P1 | `harness/scripts/shared-build.sh:147-171,255-267` | Closed: process-group cancellation and normal-exit drain pass |
| F002 | P2 | `harness/scripts/shared-build.sh:103-117` | Remains blocking: missing target plus trailing slash selects a different lock |

### F002: strip the unresolved target's trailing slash before deriving the lock

For an initially nonexistent `/tmp/example/target/`, `physical_target` preserves the final slash in `rest`. The returned target still ends in `/`, so its lock is `/tmp/example/target/.lock`. The new parent-creation step creates `target` before cargo starts.

A second invocation against `/tmp/example/target` now resolves the existing directory with no trailing slash and takes `/tmp/example/target.lock`. Both cargo commands write into the same physical target under different locks.

The fake-cargo reproduction removes an empty fixture target, starts the first invocation with the slash and waits for cargo to start, verifies the interior `.lock`, then starts the second invocation without the slash. Both return zero with:

```text
start first
start second
end second
end first
```

The exported targets differ only by their final slash. This is the same target-identity failure as F002, in the newly added nonexistent-target normalization path. The original implementation stripped a trailing slash before deriving the lock; the repair replaces that step without ensuring the reconstructed suffix is normalized.

**Required:** ensure the final reconstructed target has one normalized spelling, including trailing separators, before choosing its lock and exported target. Add a focused concurrent regression for an initially absent target with `/` versus the same directory without it.

## Proven closures and controls

Thirteen repair-probe groups pass under macOS `/bin/bash` 3.2:

- Original TERM and INT reproducers now prove the old worker is dead before the second batch enters. Wrapper exits remain 143 and 130.
- Successful cargo exit cannot leave an ordinary descendant running past unlock.
- A worker that ignores TERM stays protected by the lock during the grace period, then is killed; the second caller enters afterward. The bounded two-caller probe completes in 12.76 seconds.
- Existing directory and symlink now serialize and export the same physical target.
- Missing targets under a symlinked existing parent work; dangling targets, regular-file targets and a regular-file lock obstruction refuse by name.
- Ordinary serialization, recipe quoting, environment/source refresh, failure status, pre-lock/post-acquisition floor checks, live timeout and stale-lock preservation still pass.
- Independent mutations of non-exclusive locking, process-group cleanup and canonical target identity are each detected by the corresponding oracle.

The separate fourteenth focused probe reproduces the residual F002 above. No real cargo, Rust compilation, shared live target, installed dependencies, product edits, commits or pushes.

## Exact-head evidence

- Reviewed only the two-file delta `e13a834..30b3a37` in the new detached `fleet-review-121b` worktree. Both old and new worktrees remain clean and at their assigned heads.
- `/bin/bash -n harness/scripts/shared-build.sh`: pass.
- `git diff --check e13a834de99fe8e41e0373bc3ee0e2edb4956c8e..30b3a371067450e2275d3e3926736a8479a70d0c`: pass.
- `gh run view 41787685412 --json headSha,conclusion`: exact head, success. All four jobs pass.
- `gh run view 41787685412 --log`: committed `shared-build.test.ts` is 10/10 on Node 22 and 24.
- `harness boot --json` from the new worktree: dependency advisory, `ready: false`.
- `harness checks --quick --json` from the new worktree: six dependency/tooling failures; native Copilot contracts pass; Rust/smoke skipped per the no-real-cargo packet. Not a local full-gate pass.

Retained alongside this report: `pr121b-probes.py`, `pr121b-probes.json`, `pr121b-trailing-slash.py`, `pr121b-trailing-slash.json`, `pr121b-checks.json`. Run the two Python scripts together from this output directory; the focused script loads the fixture definitions from its sibling. Scratch fixture directories are removed.

## Cost and harness feedback

No daemon change. Ordinary holding has no new steady polling. The drain loop is transient, one `/bin/sleep 0.1` per nonempty command group; probes and signals are Bash builtins. The independent performance role owns its cost verdict. The disk floor remains a one-directional brake, not a deletion/retention policy; stale locks remain untouched.

**magicWand:** add the initially-missing/trailing-slash target case to the existing exclusion test. Checking target spelling only after directory creation misses the transition that changes which sibling lock is selected.

**difficulties:** intended missing JS dependencies require exact-head CI for committed TS proof. Retained standard-library fake-cargo probes cover the missing process-group and normalization boundaries without using real cargo.
