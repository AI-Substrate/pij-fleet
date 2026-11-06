# PR #134 final repair review

## Verdict

**pass-with-notes**, exact head `39307919c43cfb23f8a5728b371234025ce37d1c`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/134#issuecomment-6674876825

F001 and N001 are closed. No remaining blocking correctness finding in the reviewed repair. This supersedes the block at `d85c0f3`; the UTC/DST portion of that review carries forward unchanged.

Complete nine-file `d85c0f3..3930791` delta reviewed in the fresh detached worktree, including the registration observer, batch collector/parser/runner, tests, tripwire and revised evidence/budget wording. Source plus exact-head CI only under the fleet backoff. No local builds, tests, process probes, mutation runs, boot/checks, daemon scripts or deployment.

## F001 closure: registration-scoped argv batch

- Both production entry points create `native_observer()` once per registration. The macOS observer owns one `BatchedAncestry` behind its per-registration mutex, not one reader per ancestor or global mutable cache.
- `BatchedAncestry::observe` reads each requested parent/name in-process. Non-host names do not invoke the argv adapter. The first host-named observation collects bounded eligible ancestor PIDs and initializes `read` from one batch; later host observations use that result, including an empty result, rather than launching again.
- The production adapter constructs one absolute `/bin/ps -ww -p <comma-separated-pids>` command with the existing spawn-counter operation. The collector selects only host-named PIDs, at most 32 from its starting host; it is not a machine-wide census. The runner retains the one-second child timeout and bounded output, now scaled by the bounded PID count.
- Batched output includes a PID column and is mapped back to the requested process by PID. Missing processes remain absent. The native/paneless matchers, exact session checks, outer 32-process/cycle guards, parent-incarnation checks and start-time sandwich are unchanged.
- CI now exercises both original counterexample shapes: paneless `pij -> node helper -> node Claude host`, and native `copilot -> node launcher -> pane zsh`. Both preserve the accepted host and assert one batch with both eligible PIDs. The six single-candidate harness/native-versus-node variants also pass.

The bounded prefetch is separate from the existing accepted-walk cap: it starts at the first host-named ancestor. No claim is made that there are only 32 kernel calls in total. The relevant launch contract is at most one registration-argv ps invocation; unrelated pre-existing liveness fallback sites are not reclassified or hidden.

**Proof grade:** the two Linux chain tests use the real spawn-counter API in an injected fixture batch and isolate the counter in a child test process. They prove registration/cache behavior and one adapter invocation; they do not themselves launch macOS ps. The source adapter performs one command per invocation. Platform execution evidence is qualified below.

## N001 closure: production delegation tripwire

`registration_gate_findings` now requires:

- the operation is recorded only in `read_argv_batch_with_ps`;
- only `native_observer` names that launch adapter;
- `native_observer` constructs `BatchedAncestry::new`;
- `observe` contains the host-name gate.

The passing CI guard runs two negative controls over source text: bypassing the constructor with a direct read, and replacing the observer body with the direct-return shape from the review. Both must produce findings while the helper remains present. This closes the reported false-negative shape. It remains a literal-source tripwire, not a proof against all possible alternative spellings; no broader claim is needed.

## Other behavior and cost

The UTC helper, Cargo manifests and lockfile are unchanged from the reviewed d85c0f3 head. No unsafe or dependency is added. Linux still reads `/proc` directly. No new loop, timer or launch site is introduced.

Steady state: the pending-Claude-seat UTC-offset loop has zero date launches. Registration argv observation is request-scoped and at most one counted ps batch on macOS, zero on Linux, independent of how many eligible ancestors occur in the bounded chain. Host-name selection remains a one-directional brake restricting argv reads. The existing timeouts, process-incarnation checks and exact-PID scope remain conservative.

## CI and macOS evidence

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42146953950

Exact reviewed SHA; all four jobs pass: Node 22, Node 24, rust-lint and rust-test. Rust summary: **1,899 passed, five skipped**. Published JUnit has 1,899 cases, zero failures/errors and confirms passes for:

- both new multi-host-chain tests;
- the batch PID-mapping parser test;
- the updated registration gate guard, including its two negative controls;
- the retired-date guard;
- the six-variant paneless gate test;
- both 32-process cap controls and the native start-time recheck;
- UTC parity and multithreaded DST controls.

CI is Ubuntu. It compiles/exercises the generic batch/parser and injected registration cases, not the macOS `native_observer`/real ps adapter. The coder's committed evidence and PR body report one macOS build under the fleet-190 lock: clippy with all targets, 92 registration tests including the real ancestry/at-most-one-launch case, and six spawn-site tests. Those are reported platform receipts inspected by this reviewer, not independent reviewer execution or hosted CI Mac proof.

The performance AFTER reading explicitly waits for merge and bounce. It is not claimed complete and is not substituted with fixture counts or the earlier private-daemon transcript.

Evidence collection used read-only git inspection and GitHub run/job-log/JUnit downloads. No local validation was executed. Final GitHub/worktree SHA matches `3930791`; PR is CLEAN and all requested CI is green.

## Handover and harness gift

No correctness changes are owed at this head. Performance retains the post-merge/bounce AFTER reading. Backoff remains in effect; no reviewer build window or new seat is requested.

**magicWand outcome:** the two previously missing multi-candidate chains and the direct-return tripwire negatives are now executable CI controls. Keep the fixture-versus-platform proof labels explicit so a green Linux counter test is never mistaken for a live macOS process measurement.

**difficulties:** none required a local workaround. The platform compilation ceiling is recorded openly, with the supplied Mac receipt kept distinct from reviewer CI evidence.
