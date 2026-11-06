# PR 134 in-process probes review

Tags: reviewer, UTC, DST, ancestry, process-cost, C27, CI-only, source-guards

Hook: a gate limits which ancestors launch; it does not limit how many ancestors pass the gate.

Verdict: block at `d85c0f3adab39895dd6776422aaa5b3ad4048d8c`. Report: `../outputs/2026-11/pr-134-verdict.md`.

F001 P2: the plan and packet require at most one ps per registration, but read_native_process_from launches independently for every host-named ancestor. A valid native copilot -> node launcher -> pane shell chain reads argv twice; paneless pij -> node helper -> recognized node host does likewise. The new count test traverses the actual registration/gate helper, but every fixture has exactly one candidate. Fix by registration-scoped/batched eligible-PID argv reads while preserving identity/nearest-host checks, or get an explicit revised budget and reconcile the claims. C27's recorded text says about one; the current plan says at most one.

N001 P3: the source guard collects the function containing read_argv_with_ps and separately sees HOST_NAMES.contains in the helper. Replacing the wrapper body with direct read_argv_with_ps(pid), while leaving the helper, preserves all its assertions. This is a source counterexample to that guard, not an executed mutation or a claim that the full gate passes. Add that exact negative control and check production delegation.

UTC conversion follows the existing ADR 0001 time/local-offset path, DST fixture and date parity pass in CI. Parent/name macOS short-info pid check and host-name lengths look correct; no unsafe, dependency or lockfile change. Both native/paneless 32-process caps, incarnation sandwich, pane reachability and nearest-host/session checks are unchanged.

Exact-head CI run 42143552183: all four jobs pass, nextest 1,896 passed/five skipped. Downloaded JUnit has 1,896 cases, zero failures/errors; UTC, gate/count, cap and spawn-site controls are present. CI is Ubuntu: macOS-only parity/real-ancestry cases are not run there. Historical coder Mac receipts are not independent reviewer proof.

No local builds, tests, process probes, mutation runs, boot/checks, daemon scripts or new seats. Read-only worktree remains frozen; no product edits, commits, pushes or deployment. #131 stays independently pending, with F004 state-printing withdrawn.

magicWand: vary candidate count in the cost fixture; add the two-node chain before claiming a registration-wide budget.

difficulties: gh run view --log did not provide named tests on the first fetch. Raw job logs supplied a positive content control and summary; the published JUnit artifact supplied actual test names. Both are the same CI origin. Source plus CI can support a verdict without a local workaround, provided the platform proof ceiling is explicit.

## Final repair at 3930791

Pass-with-notes at `39307919c43cfb23f8a5728b371234025ce37d1c`. Complete nine-file delta reviewed; F001 and N001 closed. Report: `../outputs/2026-11/pr-134-fix-verdict.md`.

Production native/paneless entry points each create one native_observer with a registration-scoped BatchedAncestry. First host-named observation initializes one bounded exact-PID ps batch; later observations use the cache. Existing acceptance caps, nearest-host/session checks and incarnation sandwich remain. The new source guard requires production batching delegation and rejects both supplied direct-read negative controls.

Exact-head CI run 42146953950 is all green: 1,899 passed/five skipped. JUnit confirms multi-node paneless/native cases, PID parser, tripwire negatives, caps and UTC/DST controls. Fixture batches record the actual counter API but do not launch macOS ps: this proves one adapter invocation. Source proves one command per adapter call. Coder/PM separately report the Mac build (clippy, registration 92, spawn_sites 6 including real ancestry); that is not independent reviewer execution. Live AFTER waits for merge/bounce and remains performance-owned.

No local builds/tests/probes/mutations/scripts, product edits, commits, pushes or deployment. Final GitHub/worktree pin matches, CLEAN. The prior UTC review carries forward; no unsafe/dependency changes.

magicWand outcome: multi-candidate chain pressure and exact bypass-negative controls now run in CI. Keep generic-fixture, platform and live-fleet evidence distinct.

## Amended C27 and final c54aeb5

Pass-with-notes at `c54aeb51438ef0f20845ec5e7d560a7a2fefc02c`, superseding 3930791 and 61a1c8d. Report: `../outputs/2026-11/pr-134-c54aeb5-verdict.md`.

The prime amended canonical C27 after actual Mac measurements: multi-PID ps costs about 0.6 s kernel CPU, so never batch/census. Current contract is at most one cached single-PID args-only ps per host-named ancestor, PID-bound and fail-closed. The old F001 one-per-registration demand is superseded, not re-raised. Do not optimize this back into a batch.

AncestryReads caches per PID for one registration. Non-hosts launch none. Single-pid ps emits only pid/args; parser refuses another PID, second/duplicate row or malformed/argv-less output. In-process name/parent recheck after argv read rejects observed changes; old matchers, cap and incarnation sandwich remain. The delegation guard and both negatives remain, so N001 stays closed.

Exact-head CI run 42156132234 all green, 1,900 passed/five skipped. JUnit confirms expected native/paneless counts 2/2, host-only reads, shell no-read, parser/exec/cap/UTC controls. Counters are fixture callback proof, not Linux execution of the Mac adapter. Author/prime report niced fleet-194 Mac clippy, registration 118, spawn_sites 6 and real-ps pair 0.19 s. The AFTER fleet reading remains performance-owned after merge/bounce.

Reviewed the feature delta and verified the only unrelated pr122 test-helper deletion matches merged main f23c34e. Worktree/head frozen and CLEAN; no local reviewer tests/builds/probes, product edits or deployment. UTC/manifests unchanged.

magicWand: encode Mac syscall CPU/time pressure alongside launch counts; fewer launches was not cheaper. Current ruling and corrected evidence must supersede the historical batch paragraphs.
