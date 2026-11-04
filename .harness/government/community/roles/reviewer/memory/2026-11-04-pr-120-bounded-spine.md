# PR 120 bounded-spine review

Tags: reviewer, spine, paging, mutation, shared-target, disk-floor

Hook: a shared-target lock is only a convention unless every builder uses it; passing overlapped results are not review proof.

Verdict: pass-with-notes at frozen `43821d5ba66ad7e6f46a976785d393f9f4af0c15`. Report: `../outputs/2026-11/pr-120-verdict.md`. No correctness defect or code fix owed. AC5 live measurement remains performance-owned.

The indexed-read mutation drops STATE_VERIFIED only from live seats' all_matching kinds. The existing oracle fails at anomalies_tests.rs:443: the indexed answer adds previously verified assignment a1 (three unverified-done rows versus two in the fold). Restored anomalies.rs is byte-identical to frozen source. After shared-target debug artifacts were cleared by the prime, an uncontended cold build of the restored daemon target passes the oracle and source no-tail guard, 2/2. Disk stays 45 GiB and the outer lock is released before reporting.

Additional scratch-only paging controls pass: 501 rows with default 500; exactly 500 rows remaining; an empty filtered project page that must still advance; absent peer; cursor beyond head. The real SQLite spine page contract passes too. Receipts and boundary diff are retained under `<reviewer-session-files>/pr120-*`.

The first six passing controls and million-row timings were withdrawn because PM confirmed a separate unlocked workspace build overlapped them. Exact-head CI run 41782033159 is all green; PM also confirmed Kestrel's locked full-suite rerun. Do not copy the initial local timings as trusted proof.

magicWand: promote the session-local shared-target verification wrapper into a canonical engineering-harness command: one lock owner, no competing Cargo process, source-root refresh, disk-floor check after waiting and before every Cargo command, whole-batch lock lifetime.

difficulties: a private wait-for-no-cargo loop let an unlocked builder bypass the shared lock; disk fell from 26 GiB to 24 and then 21 while waiting, invalidating a pre-lock floor check. The review stopped before Cargo at each refusal. Intentional missing JS dependencies mean boot is not-ready and quick checks fail six tooling stages; native Copilot contracts pass. No installation, product edit, commit, push or live-daemon operation.
