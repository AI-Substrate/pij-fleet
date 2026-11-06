# PR 121 owner-hardening delta review

**reviewer: pass-with-notes (head eff6286758eae1d8f825805f3ceda04dbf6c8ffb)**

Delta-only correctness review from 2ecbd26, in detached `fleet-review-121d`. No new correctness finding or change owed. Earlier F001/F002 closures carry forward: process-group cancellation and target normalization are unchanged.

## Proof

Nine isolated probe groups pass under macOS Bash 3.2 and `umask 002`, using fake cargo and temporary targets only:

- The default mkdir path creates a held lock with mode `0700`. Its owner remains readable by the same-user waiter; concurrent batches serialize and release the lock. Cargo failure still returns 7, releases, and skips later batch commands.
- A mkdir seam plants an existing-target symlink, dangling symlink, regular file, directory or FIFO at `owner`. Every case returns 2 with the named refusal, releases the lock, leaves the separate fixture target unchanged or absent, and launches no cargo. The seam executable's path includes a space.
- An injected mkdir failure refuses by name before cargo and leaves no lock.
- Removing `-m 0700` in a scratch mutant produces `0775`; the mode oracle detects the regression.
- Removing the owner-creation guard in a separate scratch mutant overwrites only the dummy fixture target and starts fake cargo; the refusal oracle detects both failures.

The committed two new tests exercise the requested mode and planted-symlink cases. The guards remain before source touching and cargo launch; refusal explicitly releases the held lock. The injected executable is quoted as a single command path, not shell text.

This is not a security verdict on the environment seam or a replacement for the security role's threat-model review. Security and performance remain independently owned.

## Validation and limitations

- `/bin/bash -n harness/scripts/shared-build.sh`: pass.
- `git diff --check 2ecbd26a8437593f03e27b1b7e412862d7900baf..eff6286758eae1d8f825805f3ceda04dbf6c8ffb`: pass. Only the script and its tests differ.
- `python3 <role-output-dir>/pr121d-probes.py`: nine groups pass, including the two independently detected mutations. It loads fixture definitions from sibling `pr121b-probes.py`, but explicitly invokes only the eff6286 script.
- `gh run view 42124845724 --json headSha,conclusion`: exact reviewed head, success. All four CI jobs pass.
- `gh run view 42124845724 --log`: `shared-build.test.ts` is 13/13 on Node 22 and 24.
- `harness boot --json`: advisory `ready: false`, intentionally missing reviewer dependencies.
- `harness checks --quick --json`: six dependency/tooling stages fail; native Copilot contracts pass; Rust and smoke skipped per packet. Not a local full-gate pass.

The new worktree remains clean at the frozen head. No real cargo, live shared target, product edits, dependency installs, commits or pushes; older worktrees were not moved. Fixture directories are cleaned up.

Retained here: `pr121d-probes.py`, `pr121d-probes.json`, `pr121d-checks.json`, alongside the shared earlier fixture definitions.

## Harness feedback

**magicWand:** already encoded: an after-mkdir fixture seam makes the occupied-owner refusal deterministic instead of relying on a scheduler race. Both new guards have a sensitive independent mutation control.

**difficulties:** intended missing dependencies still limit local composite checks; exact-head CI supplies committed TS proof. No new daemon or recurring poll is introduced by this delta. The existing disk-floor check remains a one-directional brake, not a deletion or retention policy.
