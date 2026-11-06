# PR #134 amended-C27 review

## Verdict

**pass-with-notes**, exact head `c54aeb51438ef0f20845ec5e7d560a7a2fefc02c`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/134#issuecomment-6676109808

The implementation conforms to **C27 as amended**: at most one cached, single-PID argv ps read per host-named ancestor; no PID batch or census. The old one-per-registration finding/recommendation is superseded by the prime's explicit amendment, not reapplied to the expected two-call Copilot chain. N001 remains closed by the production-delegation guard and its two negatives. No remaining blocking correctness finding.

Reviewed the `3930791..c54aeb5` delta, affected observer/parser/count paths, amended plan/backpressure and reported Mac receipts. The unrelated pr122 helper deletion is inherited from main f23c34e and matches main. The UTC helper and manifests/lockfile are unchanged from the previously reviewed head.

CI first. No additional reviewer local build, test, process probe, mutation run or script was needed; the reported platform receipts already cover the Mac-only adapter. Fresh detached worktree stayed clean and frozen. No product edits, commits, pushes or deployment.

## Current contract and source

The governance C27 row explicitly forbids multi-PID ps after the reported 0.6-second kernel-CPU measurement, despite its lower launch count. It permits one single-PID call per eligible ancestor, requires args-only/PID-bound fail-closed reads, and keeps unsafe forbidden.

- Both production registration entry points create one `native_observer()` with a registration-scoped `AncestryReads`. The cache is not global or shared across registrations.
- `AncestryReads::observe` obtains parent/name in-process and gates argv on the four host names. Shells launch nothing. It caches the argv result, including absence, per PID; repeated reads do not launch again.
- `read_argv_with_ps` constructs one absolute `/bin/ps -ww -p <pid> -o pid= -o args=` command. There is no comma list, census, raw ucomm or parent column. The existing counter, one-second timeout and 64-KiB output limit remain.
- `parse_argv_row` accepts exactly one nonempty row, requires its PID to equal the requested PID and requires argv. Wrong-PID, duplicate/second-row, malformed and non-UTF-8 data are refused, not substituted for another process.
- After a successful argv read, the triggering PID's parent/name are read again; an observed change refuses before caching. The native/paneless matchers, nearest-host/session checks, 32-ancestor/cycle limits and process-start sandwich remain in place.
- Linux continues to read `/proc`; no unsafe or dependency is added.

These are correctness checks of the requested PID-binding/fail-closed behavior, not a substitute for the separately owned security verdict.

## Count and guard evidence

The passing CI tests exercise:

- native Copilot plus node launcher: exactly **2** eligible argv calls, with the expected PIDs and unchanged accepted host;
- paneless two-node chain: exactly **2** calls and the same accepted Claude host;
- six native/node host variants: exactly the one host PID read;
- a shell/no-process control: no argv callback;
- wrong-PID, duplicate/newline-row and malformed/argv-less row refusals;
- observed exec/name change during argv read: refusal;
- the production `AncestryReads::new` delegation guard, with both direct-read/direct-return negative controls;
- both accepted-walk cap controls and the retained UTC/DST tests.

Fixture readers increment the actual spawn-counter API in isolated child tests; they prove callback counts/cache and registration behavior, not a physical Mac ps launch. Source ties one adapter call to one single-PID command. The reported real-ps Mac run provides separate platform evidence.

The source tripwire remains a literal-shape guard, not a proof against every alternate spelling. Its reported bypass negatives remain executable and pass CI after the rename.

## Exact-head CI and platform proof

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42156132234

All four jobs pass at the reviewed SHA: Node 22, Node 24, rust-lint and rust-test. Rust nextest summary: **1,900 passed, five skipped**. Downloaded JUnit confirms the amended count tests, PID-row controls, exec-change refusal, gate negatives, cap tests and UTC/DST controls.

Ubuntu CI exercises the generic reader/parser and injected registration cases, not the Mac-specific process adapter. The committed phase-1 evidence and current PR appendix report:

- macOS clippy, 118 registration tests and six spawn-site tests through the fleet-194 target under `nice -n 19 taskpolicy -b`;
- the prime's authorized real-ps pair passing in 0.19 seconds;
- the single-versus-multi PID cost measurements that motivated C27's amendment.

Those are inspected, attributed author/prime platform receipts, not measurements performed by this reviewer. The earlier batch approval and QoS explanation are historical and superseded by the corrected diagnosis.

Performance's fleet **AFTER** reading still waits for merge and bounce. No fixture, CI result or Mac unit-test timing is represented as that live measurement.

Final pin inspection: GitHub and the clean detached worktree match `c54aeb5`; PR is CLEAN. GitHub run metadata, job log and JUnit are views of one CI origin. No complete local gate is claimed.

## Cost and handover

The UTC loop still launches zero date processes. Registration is request-scoped: zero argv launches for a non-host ancestor, at most one single-PID argv launch for each host-named ancestor visited, cached for the registration. Typical host-only/Copilot counts are one/two, as the amended ruling and tests now state. There is no new timer, poll or whole-machine observation.

No correctness change is owed at this head. Do not restore batching to satisfy the superseded one-launch contract. N001 remains closed; performance retains the post-merge/bounce AFTER receipt.

**magicWand:** preserve a Mac single-versus-multi-PID CPU/time regression check beside the launch-count controls. This review round demonstrated why fewer launches alone cannot prove lower resource cost. Keep that platform check separately graded from generic CI.

**difficulties:** the PR contains historical batch evidence as well as the final amendment. Reading the canonical current C27 and the corrected phase-1 appendix prevented the old launch-count target from being mistaken for today's contract.
