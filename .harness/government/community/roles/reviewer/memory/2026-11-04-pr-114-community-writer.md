# PR 114 community writer review

Tags: reviewer, community, ddocs, batch, checkout-safety, mutation

Hook: ddocs exit 0 can contain a missing-brief warning; test product validity, not only validator failure.

Verdict: block at frozen `d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed`. Report: `../outputs/2026-11/pr-114-verdict.md`. F001: a missing standing_packet brief commits, including an otherwise valid two-role batch, because the writer accepts ddocs' degraded-success exit code. F002: PATH can resolve ddocs to a relative path, then current_dir(governance) changes its meaning; actual launch fails as missing. Absolute-path and existing-brief positive controls pass.

Thirty-three original targeted tests pass: community 24, directory HTTP 3, store pointer 1, spawn sites 4, ping 1. Added review controls preserve foreign index/worktree state on commit-hook failure and prove an existing brief works. The augmented community run has 26 pass, 2 fail reproducing F001/F002. Disabling the foreign-edit brake makes its regression fail; removing both launch counters makes spawn_sites name both sites. Restoring mutations restores the original passes.

All cargo commands reused the coder's target directory, never this worktree's target. Disk was 31 GiB at entry and 28 GiB near closeout. No dependency install/link, production-daemon testing, test writes to live governance, product source edit, commit or push. Exact-head CI run 41768673271 has four green jobs. Local boot is NOT-READY; quick checks fail five JS/tooling stages, Copilot-native passes. No full local green gate claimed.

The PR advanced through documentation heads while this detached worktree stayed at the assigned SHA. PM was told; never move this checkout under a reviewer. The report's scope remains exact-head.

Follow-up: PM explicitly requested checking `git diff d5fd8cf 0c6ea88` and pinning the verdict to `0c6ea8886e6f4dec9cc09d61cdcd654157be28ae`. Complete delta is two Markdown files only, recording the lifecycle-test flake and renumbering difficulty rows; no new finding. Verdict remains block and now names 0c6ea88; unchanged code/schema/test bytes retain the runtime receipts. Diff whitespace check passes and all four exact-0c6ea88 CI jobs pass (run 41769261584). No tests rerun or checkout moved. GitHub already showed e735b84 when this queued freeze message arrived; PM is told the verdict does not cover that newer head.

Final-head follow-up: PM requested b28d815. Read the entire d5fd8cf..b28d815 delta: four documentation files only, adding flake notes, measured-cost correction and coder shared-tool receipts. Backpressure validates 0/0 and build --check reports no drift in a scratch exact-b28d815 snapshot. Exact-head CI run 41769699230 has all four jobs green. Verdict remains block, now pinned to `b28d815be55ee4698d308eea1b67d91c3e4d0fd2`; no runtime code/schema/test changes or rebuild. GitHub already reports 8eb245b, outside this requested verdict. Detached review HEAD stays d5fd8cf.

Reproducible evidence is in session `e1e17389-7c15-4043-a345-a4d5df30b473/files/pr114-regression-tests.patch` and `pr114-probe-results.txt`. The patch applies to the frozen head. Temporary source extraction is disposable; the patch and output carry the evidence.

**magicWand:** a committed real-ddocs missing-link batch regression, not another fake validator that only rejects a sentinel.

**difficulties:** missing node_modules makes full local proof unavailable under the no-install packet; moving PR heads require explicit frozen-head scope. DL-001/MW-001 captured in the ignored observation buffer for PM-owned harvesting.
