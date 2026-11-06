# PR 121 correctness review

**Latest:** pass-with-notes at `eff6286758eae1d8f825805f3ceda04dbf6c8ffb`; see [owner-hardening delta verdict](pr-121-owner-fix-verdict.md). F001 and F002 remain closed. The text below is the historical initial-head review.

## A. Verdict

**reviewer: block (head e13a834de99fe8e41e0373bc3ee0e2edb4956c8e)**

Plan 198, AC1 to AC6. Two reproduced correctness defects defeat exclusion on the shared target. No product edits, commits, pushes, real cargo, or live shared-target access.

## B. Summary

Ordinary same-path batches serialize correctly. The atomic mkdir, pre-lock and post-acquisition floor checks, owner record, source refresh, argument forwarding, failure propagation, timeout and stale-lock preservation work in isolated macOS Bash 3.2 probes. All four exact-head CI jobs pass, including the eight committed tests on Node 22 and 24.

Cancellation only terminates cargo's immediate PID, leaving descendants able to write after the lock is released. Separately, a target symlink and its destination acquire different locks for the same physical target.

## C. Checklist

| Surface | Result |
|---|---|
| Atomic mkdir and ordinary whole-batch serialization | Pass |
| Failure status, environment, source refresh and recipe argument preservation | Pass |
| Floor before acquisition and immediately before cargo | Pass |
| SIGINT/SIGTERM wrapper exit and lock removal | Pass, but unsafe process-tree lifetime: F001 |
| Shared-target identity across path aliases | Block: F002 |
| Live-owner timeout and dead-owner stale reporting without deletion | Pass |
| justfile, AGENTS.md and D-198-1 | Present; unconditional safety/release claims depend on fixes |
| Exact-head CI | Four jobs pass |

## D. Findings

| ID | Priority | Location | Finding |
|---|---|---|---|
| F001 | P1 | `harness/scripts/shared-build.sh:126-132` | Do not release exclusion while cargo descendants remain active |
| F002 | P2 | `harness/scripts/shared-build.sh:89-96` | Identify the physical target before choosing its lock |

## E. Detailed findings

### F001: cancellation leaves build descendants running after unlock

`on_signal` sends TERM only to `$child` and waits only for that PID. Cargo launches compiler, linker and build-script descendants; terminating their parent is not proof those processes have stopped. The handler then deletes the lock and admits the next worktree while earlier build processes can still modify the same target.

The fake cargo probe launches a worker, waits for its start, and stays alive waiting for it. Sending TERM to the wrapper returns 143 and removes the lock, but a second invocation starts while the first worker remains active for another 1.264 seconds. INT returns 130 with the same failure, leaving 1.129 seconds of overlap. Both recorded:

```text
worker-start first
start first
start second
end second
worker-end first
```

No cargo binary or Rust compiler was used. This directly violates the no-overlap goal and AC1/AC4 when a build is cancelled.

**Required:** contain and stop/reap the command's process tree before releasing its lock, using a macOS-compatible mechanism. Cover both INT and TERM with a fake cargo that launches an observable worker. Assert that no old worker can keep writing after another invocation acquires the target, not merely that the wrapper exits and the lock disappears.

The existing fake deliberately redirects its orphaned sleep away from the test's pipes (`shared-build.test.ts:69-70`), and the signal test only checks wrapper status and lock absence (`:194-202`). Thus the committed green test does not prove process-tree exclusion.

### F002: a target symlink bypasses serialization

The script requires an absolute target but only removes one trailing slash before deriving `$target.lock`. Given an existing directory `target` and a symlink `alias -> target`, two callers can set the target to those two absolute paths. Their cargo outputs share one directory, but their sibling lock directories are distinct.

The probe starts a slow batch against the real target, waits until cargo starts, then starts another batch against its symlink. Both wrappers succeed with:

```text
start first
start second
end second
end first
```

**Required:** canonicalize the target's filesystem identity before deriving both the lock and exported target, including the supported nonexistent-target case, or explicitly refuse target spellings that cannot be safely normalized. Add a real-directory/symlink exclusion test. The documented interface accepts these absolute paths today; an unstated common-spelling convention is not an exclusion guarantee.

## F. Acceptance coverage and cost

| AC | Disposition |
|---|---|
| AC1 | Partial: normal execution passes, cancellation unsafe (F001), alias exclusion unsafe (F002) |
| AC2 | Proven in focused fake-df controls, including space falling between the pre-lock and pre-command checks |
| AC3 | Live timeout and stale refusal pass; existing tests do not cover alias identity |
| AC4 | Existing eight tests are green, but process-tree and alias overlap remain unguarded |
| AC5 | Recipe is listed; requested docs are present |
| AC6 | No daemon change; nothing at rest; transient waiter loop is 20 seconds per caller, with one df, mkdir and sleep each poll; performance role owns its independent verdict |

The disk floor is a **one-directional safety interlock (a brake), not a policy**: removing it can only permit more build work. Stale-owner detection never deletes a stale lock; automatic deletion is limited to the lock this invocation believes it owns. Neither issue requires a retention or ageing ruling.

## G. Commands and retained evidence

- `/bin/bash --version`: 3.2.57; `/bin/bash -n harness/scripts/shared-build.sh`: pass.
- `just --list`: includes `shared-build`.
- `python3 <role-output-dir>/pr-121-probes.py`: eight probe groups complete. Ordinary serialization passes; a non-exclusive-lock mutation produces overlap and is detected; TERM, INT and symlink-target overlap defects reproduce. Recipe forwarding preserves spaces, apostrophes, an empty argument and literal shell metacharacters.
- `gh pr view 121 --json headRefOid,statusCheckRollup`: exact reviewed head; `check (22)`, `check (24)`, `rust-lint`, `rust-test` all SUCCESS in run 41785676229.
- `gh run view 41785676229 --log`: confirms `harness/scripts/shared-build.test.ts` passed 8/8 on both Node versions.
- `harness boot --json`: advisory `ready: false`, missing worktree dependencies.
- `harness checks --quick --json`: fails six tooling/dependency stages; native Copilot contracts pass; Rust and smoke deliberately skipped. This is not a local full-gate pass. No dependencies installed or linked, per packet.

Retained alongside this report: `pr-121-probes.py`, `pr-121-probes.json`, `pr-121-checks.json`. All temporary fixture directories are removed by the probe runner after their bounded workers finish. Product worktree remains clean at the pinned head.

## H. Handover and harness feedback

Fix F001 and F002, extend the existing fake-cargo suite with the reproduced cases, then request exact-head re-review. Do not treat the current green CI or the original non-exclusive-lock mutation as proof of these missing invariants.

**magicWand:** make the signal fixture own an observable descendant that writes until stopped; cancellation must prove the old build tree is quiescent before unlock. That one stronger sensor would have rejected F001 automatically.

**difficulties:** local TS validation is intentionally dependency-limited; exact-head CI supplies the committed-test evidence. Scratch-only standard-library probes supplied the missing process-tree and target-identity evidence without real cargo or product edits. The executable reproducer is retained for incorporation into the existing tests by the coder.
