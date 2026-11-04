reviewer: pass-with-notes (head 43821d5ba66ad7e6f46a976785d393f9f4af0c15)

No correctness findings or code fixes required. This covers the exact frozen head of PR #120 against `d37da2f1cf7b06c154768123ba3d7e7b87d325e0`. Notes are proof boundaries, not defect findings: the local composite gate is tooling-limited, and live AC5 measurements remain performance's responsibility.

## Completed evidence

- Read the exact-head diff against `d37da2f1cf7b06c154768123ba3d7e7b87d325e0`, plan 197 AC1-AC6, task assertions, bounded-read contracts, dependent Spine implementations and the previous anomaly fold.
- AC1: `node_show` uses `newest()`; its remaining projection is unchanged.
- AC2: default 500, maximum 5000, named refusals, one extra lookahead row, exclusive keyset cursors and indexed `--peer` reads are implemented. The committed page test covers three pages, peer pages, render and invalid limits; exact-head CI and Kestrel's locked full-suite rerun pass. Additional independent scratch boundary controls pass. `--project` filters after reading a bounded page, so an empty result may still carry `more: true`.
- AC3: the old fold's decoding moved without a semantic change. The equivalence oracle passes unscoped, two folders, two seat scopes and a negative project scope. It includes stale/fresh/parked/skewed/garbled cards, tombstoned done claims, two-incarnation seat.put histories, malformed newest seat.put fallback, dispatch evidence, open questions, dispositions and junk history.
- AC4: the committed million-row fixture seeds 1,000,073 rows. Query plans require SEARCH and reject SCAN spine_events; the runtime and source guards reject tail. The large fixture correctly lives outside src because the direct-insert guard covers every crate's src tree. This reviewer does not reuse the overlapped initial timing as proof: the large-fixture pass is supported by exact-head CI and PM's confirmation of Kestrel's locked full-suite rerun.
- Caller audit included hidden .omp paths, crates, harness and skills. No production programmatic caller assumes the entire history arrives in one call. The smoke callers use isolated, small fresh stores. The API and node/store-native skill instructions document paging.
- AC5 belongs to performance; no live deployment or cost verdict attempted here. No timer, polling frequency or process-launch site is added. Anomalies does add per-request per-seat reads; all_matching still costs the selected kinds' retained rows, not a constant per seat.
- AC6: issue closure, D-197-1, API/skill documentation and the separate junk-row operator decision are present.
- Exact-head CI run `41782033159`: all four jobs pass; API confirms its head SHA is the reviewed SHA.

The first local baseline is **withdrawn as review proof**: PM confirmed an unlocked builder overlapped it despite this review holding the outer lock. The exact-head CI evidence is unaffected. Independent mutation, paging boundaries, SQLite contract and restored-oracle evidence below were run in uncontended locked batches.

Accepted independent commands, with shared target and CARGO_INCREMENTAL=0 for Cargo:

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --lib indexed_reads_answer_exactly_as_the_whole_spine_fold -- --nocapture` | EXPECTED FAIL on scratch mutation, one assertion failure, exit 101 |
| `cargo test --locked -p pij-daemon --lib reviewer_paging_default_exact_boundary_and_empty_project_page -- --nocapture` | PASS, one scratch-only boundary test |
| `cargo test --locked -p pij-store --test store_integration the_sqlite_spine_honours_the_same_page_contract_the_fake_does -- --nocapture` | PASS, real SQLite page contract |
| `cargo test --locked -p pij-daemon --lib http::anomalies::tests:: -- --nocapture` | PASS after restoration, 2 tests, fresh cold build from frozen checkout |
| `git diff --check` | PASS |
| `harness boot` | NOT-READY: intentionally absent review-worktree JS dependencies |
| `harness checks --quick` | FAIL: six tooling-dependent stages; native Copilot contracts pass; Rust/smoke skipped |

No full local gate is claimed. Exact-head CI covers the full workspace, Rust lint and JS checks. Local fmt/clippy were not repeated: PM narrowed the final run to one daemon target to protect shared disk. `gh pr checks 120` reports all four jobs passing, and the Actions API confirms the exact SHA.

## Mutation and restored proof

On an immutable-head scratch archive, removed STATE_VERIFIED only from the indexed live-seat kind list. Under an uncontended outer lock, the oracle failed exactly at anomalies_tests.rs:443 (exit 101, one failed test): the indexed answer had assignment a1 and three unverified-done rows; the old fold had no a1 and two such rows. This detects a real missing indexed kind rather than a compilation failure. Restored anomalies.rs is byte-identical to the frozen checkout.

The same clean batch passed an additional scratch-only boundary test covering default 500 over 501 rows, exactly 500 remaining rows, an empty project-filtered page followed by its match, an absent peer and a cursor beyond the head. It also passed the real SQLite spine page contract, whose name the initial baseline selector missed.

Receipts: `<reviewer-session-files>/pr120-mutation.txt`, `pr120-boundary.txt`, `pr120-boundary.diff`, `pr120-sqlite-contract.txt`, `pr120-restored-anomalies.txt`.

After the prime cleared shared build artifacts, the final restored run acquired the outer lock at 16:13, confirmed no other Cargo process and 45 GiB free, compared restored anomalies.rs byte-for-byte to the frozen checkout, refreshed crate source roots and cold-built the one daemon target. Both equivalence and no-tail tests passed (2/2). No other Cargo process appeared at completion, disk remained 45 GiB, and the lock was released. PM was notified immediately so the next builder could proceed.

The original review worktree remains clean at the frozen SHA. No product edits, commits, pushes, installs, production-daemon changes or process teardown.

## Harness feedback

magicWand: one shared-target build wrapper should own serialization, the in-lock disk check, source refresh and whole command batch. During the baseline batch cargo reported internal build-lock waits despite this review owning the outer lock; PM later confirmed an unlocked builder, so the first baseline was withdrawn. The disk-floor recheck must happen after waiting as well as before it. This review encoded those checks in a session-local verification wrapper; promoting that wrapper belongs in the engineering harness, not this frozen product review.

difficulties: shared-target contention invalidated the first passing baseline; disk crossed to 24 GiB and then 21 GiB while waiting, so attempted reruns stopped before Cargo; missing JS dependencies intentionally left untouched. Observations and this retrospective are preserved for PM curation. The best encoding is a shared-target wrapper that enforces the lock and rechecks the disk floor under it, rather than relying on every seat's private waiting loop.
