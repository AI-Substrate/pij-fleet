# PR 121 final correctness review

**Later head:** the [owner-hardening delta review](pr-121-owner-fix-verdict.md) passes with notes at `eff6286758eae1d8f825805f3ceda04dbf6c8ffb`. This file records the earlier normalization-fix review.

**reviewer: pass-with-notes (head 2ecbd26a8437593f03e27b1b7e412862d7900baf)**

F001 and F002 are closed. No remaining correctness finding or code change owed. This supersedes both earlier blocking verdicts.

## Closure evidence

F002's exact residual reproducer now passes: begin with no target directory, start `target/`, then start `target` while the first cargo is active. Both export the same physical target, use the same sibling lock, and record:

```text
start first
end first
start second
end second
```

The second caller reports waiting; no interior `target/.lock` is created; the shared sibling lock is released afterward.

Five independent concurrent controls pass under macOS Bash 3.2: the original missing-target/trailing-slash case; missing target with doubled/trailing separators; existing target with doubled/trailing separators; missing suffix under a symlink with repeated separators; and an existing target symlink with trailing separators.

A scratch-only mutation removes both new normalization calls. The same oracle detects the original failure: overlapping `start first; start second; end second; end first`, different exported spellings, and an interior lock. The instrument therefore rejects the broken behavior rather than merely passing the repaired script.

F001's process-group closure carries forward from the 30b3a37 review: TERM, INT, normal-exit leftovers and a TERM-resistant worker were independently verified there. The final delta changes only path normalization and adds its focused test; cancellation code is unchanged. The final exact-head committed suite includes both signal regressions.

## Scope and validation

- Reviewed the entire two-file `30b3a37..2ecbd26` delta in the new detached `fleet-review-121c` worktree. Original and first-repair worktrees remain pinned at e13a834 and 30b3a37; product worktrees are unchanged.
- `/bin/bash -n harness/scripts/shared-build.sh`: pass.
- `git diff --check 30b3a371067450e2275d3e3926736a8479a70d0c..2ecbd26a8437593f03e27b1b7e412862d7900baf`: pass.
- `python3 <role-output-dir>/pr121c-probes.py`: five positive concurrent cases and one detected normalization mutation pass. It loads fixture definitions from sibling `pr121b-probes.py`, but explicitly invokes only the final-head script.
- `gh run view 41788830682 --json headSha,conclusion`: exact reviewed head, success. `check (22)`, `check (24)`, `rust-lint` and `rust-test` all pass.
- `gh run view 41788830682 --log`: committed `shared-build.test.ts` is 11/11 on Node 22 and 24, including the initially-missing `target/` concurrency regression.

**Notes:** local boot remains `ready: false` because this reviewer worktree intentionally has no dependencies. `harness checks --quick --json` fails six dependency/tooling stages; native Copilot contracts pass; Rust and smoke are skipped per packet. This is not a local full-gate pass. Exact-head CI supplies the committed TS/gate evidence. Security and performance retain their independent verdicts.

No real cargo, live shared target, product edits, dependency installs, commits or pushes. Temporary fixture directories are removed.

Retained here: `pr121c-probes.py`, `pr121c-probes.json`, `pr121c-checks.json`; earlier fixture/probe files remain alongside them.

## Harness feedback

**magicWand:** already encoded by this repair: the committed missing-target/trailing-slash concurrency test turns a normalization transition into deterministic exclusion proof. The retained regression mutation confirms it observes the relevant invariant.

**difficulties:** intended missing dependencies limit local composite checks; no new workaround is needed. Normalization uses Bash builtins and adds no per-poll work. No daemon change; performance owns its cost verdict. Disk-floor refusal remains a one-directional brake, not a deletion or retention policy.
