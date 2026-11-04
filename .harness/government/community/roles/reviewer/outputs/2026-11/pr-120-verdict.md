# PR 120 correctness review

Status: **incomplete, verification paused** at head `43821d5ba66ad7e6f46a976785d393f9f4af0c15`. No correctness defect found in the reviewed diff. This is not a pass or merge approval.

## Completed evidence

- Read the exact-head diff against `d37da2f1cf7b06c154768123ba3d7e7b87d325e0`, plan 197 AC1-AC6, task assertions, bounded-read contracts, dependent Spine implementations and the previous anomaly fold.
- AC1: `node_show` uses `newest()`; its remaining projection is unchanged.
- AC2: default 500, maximum 5000, named refusals, one extra lookahead row, exclusive keyset cursors and indexed `--peer` reads are implemented. The committed page test passes across three pages, peer pages, render and invalid limits. `--project` filters after reading a bounded page, so an empty result may still carry `more: true`.
- AC3: the old fold's decoding moved without a semantic change. The equivalence oracle passes unscoped, two folders, two seat scopes and a negative project scope. It includes stale/fresh/parked/skewed/garbled cards, tombstoned done claims, two-incarnation seat.put histories, malformed newest seat.put fallback, dispatch evidence, open questions, dispositions and junk history.
- AC4: the million-row test passes over 1,000,073 rows. Anomalies: 16.925667 ms; head plus two 5001-row pages: 35.725375 ms, each within 2 s. Query plans require SEARCH and reject SCAN spine_events; the runtime and source guards reject tail. The large fixture correctly lives outside src because the direct-insert guard covers every crate's src tree.
- Caller audit included hidden .omp paths, crates, harness and skills. No production programmatic caller assumes the entire history arrives in one call. The smoke callers use isolated, small fresh stores. The API and node/store-native skill instructions document paging.
- AC5 belongs to performance; no live deployment or cost verdict attempted here. No timer, polling frequency or process-launch site is added. Anomalies does add per-request per-seat reads; all_matching still costs the selected kinds' retained rows, not a constant per seat.
- AC6: issue closure, D-197-1, API/skill documentation and the separate junk-row operator decision are present.
- Exact-head CI run `41782033159`: all four jobs pass; API confirms its head SHA is the reviewed SHA.

Commands run with the shared target, CARGO_INCREMENTAL=0 and the outer mkdir lock:

| Command | Result |
|---|---|
| `cargo test --locked -p pij-daemon --lib http::anomalies::tests:: -- --nocapture` | PASS, 2 tests |
| `cargo test --locked -p pij-daemon --lib spine_events_pages_by_cursor_and_never_reads_the_whole_spine -- --nocapture` | PASS, 1 test |
| `cargo test --locked -p pij-daemon --test spine_attach_cost anomalies_node_show_and_spine_events_stay_bounded_over_a_million_rows -- --nocapture` | PASS, 1 test |
| `cargo test --locked -p pij-store --test store_integration seat_latest_page_and_latest_matching_seek_their_index -- --nocapture` | PASS, 1 test, before/after ANALYZE |
| `cargo test --locked -p pij-store -p pij-testkit spine_page -- --nocapture` | PASS, fake contract only; this selector did not match the SQLite contract |
| `git diff --check` | PASS |
| `harness boot` | NOT-READY: intentionally absent review-worktree JS dependencies |
| `harness checks --quick` | FAIL: six tooling-dependent stages; native Copilot contracts pass; Rust/smoke skipped |

No full local gate is claimed. Exact-head CI covers the full workspace, Rust lint and JS checks.

## Pending proof and blocker

The packet requires an indexed-read mutation followed by restored success. An immutable-head scratch archive is prepared with STATE_VERIFIED removed only from the indexed live-seat kind list. The expected failure is the existing oracle emitting an extra previously verified done claim. Additional scratch-only boundary controls cover default 500 over 501 rows, exactly 500 remaining rows, an empty project-filtered page followed by its match, an absent peer and a cursor beyond the head.

**None of those scratch controls has run.** The mutation batch was stopped while waiting for the outer lock, before any cargo command. Free disk fell from 26 GiB to 24 GiB, below the packet's 25 GiB floor. At that observation the lock belonged to pij-swift-kestrel. PM was notified. Resume only above the floor, recheck disk after acquiring the lock, force-refresh source roots, prove the mutation fails for the expected assertion, restore source bytes and rerun successfully.

The original review worktree remains clean at the frozen SHA. No product edits, commits, pushes, installs, production-daemon changes or process teardown.

## Harness feedback

magicWand: one shared-target build wrapper should own serialization, the in-lock disk check, source refresh and whole command batch. During the baseline batch cargo reported internal build-lock waits despite this review owning the outer lock; that is evidence of contention, not proof of another seat's identity or of result corruption. The disk-floor recheck must happen after waiting as well as before it.

difficulties: shared-target contention; disk crossing the floor while waiting; missing JS dependencies intentionally left untouched. Observations are captured for PM curation.
