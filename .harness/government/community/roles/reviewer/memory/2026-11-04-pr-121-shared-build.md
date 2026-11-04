# PR 121 shared-build review

Tags: reviewer, shared-target, cancellation, process-tree, symlink, mutation

Hook: lock removal after parent death does not prove descendant quiescence; a textual absolute target is not a filesystem identity.

Verdict: block at e13a834de99fe8e41e0373bc3ee0e2edb4956c8e. Report: `../outputs/2026-11/pr-121-verdict.md`; retained executable probe and JSON receipts alongside it.

F001 P1: INT and TERM kill only cargo's PID, wait only for that process and remove the lock while descendants continue. Fake cargo starts a worker and waits for it; after signalling the wrapper, another invocation starts more than a second before the old worker ends. Existing tests deliberately detach the orphaned sleep's pipes and assert only status/lock absence, so 8/8 green misses the safety property.

F002 P2: target is only stripped of a trailing slash. An absolute symlink to the target gets a distinct sibling lock. Two invocations against directory and symlink overlap while writing the same physical directory. Canonicalize identity or refuse unnormalizable paths and cover it in the suite.

Ordinary serialization, a detectable non-exclusive-lock mutation, recipe argument quoting, environment, source mtimes, failure status, pre-lock and post-acquisition floor refusal, live-owner timeout and stale-lock preservation pass under Bash 3.2. Exact-head CI run 41785676229 is four green jobs with the committed eight tests on Node 22 and 24.

magicWand: replace signal-only lock absence with an observable fake build descendant whose writes must stop before another batch enters.

difficulties: no node_modules by packet, so boot not-ready and quick checks fail six tooling stages; native Copilot contracts pass. No installs, real cargo, product edits, commits or pushes. Temporary fixtures cleaned after workers finish.
