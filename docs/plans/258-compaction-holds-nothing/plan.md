# Plan 258 — Settle `compaction_wakes` as `holds-nothing` without waiting `COMPACT_SETTLE_MS` (Outpost 120 P6)

## Context & Evidence

In live fleet data (`~/.pij-rs/pij.sqlite`, 7-day window, `n = 141` `compaction_wakes` rows):
- **80.1% (113 / 141)** of compactions settle as `holds-nothing` with `min = 301.7 s`, `p50 = 341.7 s` (`COMPACT_SETTLE_MS` = 300 s + 0–60 s PA watchdog tick).

## Root Cause

1. Pre-compact memory turns (`crates/daemon/src/http/memory_turn.rs::turn_for`) already skip seats that held no role in their current context.
2. Every admitted compact is still recorded in `compaction_wakes` (`crates/daemon/src/http/mod.rs::admit_control`) — intentionally, because `CONTEXT_CUTOFF_SQL` (`crates/store/src/community.rs`) only advances a seat's memory-turn context cutoff once a compact is **delivered** and settled (`woken`, `resumed`, or `holds-nothing`), so refused/undelivered compacts never clear released roles (`a_refused_compact_never_advances_the_memory_turn_cutoff`).
3. However, in `crates/daemon/src/compaction_wake.rs::step`, the holdings and open-claims check (`try_holdings_of` + `open_claims`) ran **after** the `COMPACT_SETTLE_MS` (5 min), `SystemState::Idle`, and `queue.peek` gates. As a result, 80.1% of delivered compactions sat pending for 5+ minutes before settling `holds-nothing`, delaying `CONTEXT_CUTOFF_SQL` advancement and repeating indexed reads across ~5 watchdog ticks.

## Decision

Move the holdings/claims check in `crates/daemon/src/compaction_wake.rs::step` to run right after delivery confirmation and `started_since`, **before** the `COMPACT_SETTLE_MS`, `Idle`, and `queue.peek` waits:

1. Seat tombstoned → `Settled::Gone`.
2. Compact not delivered yet → wait up to `COMPACT_DELIVERY_BOUND_MS`, then `Settled::Undelivered`.
3. `started_since` is true → `Settled::Resumed`.
4. Read `try_holdings_of` + `open_claims`:
   - On read error → wait (never settle `holds-nothing` on a store error; preserves #167 F001).
   - If empty → settle `Settled::HoldsNothing` immediately (`delivered: true`, so older pending compactions are superseded and `CONTEXT_CUTOFF_SQL` advances on the first round after delivery).
5. Otherwise (seat holds an open role or claim): wait while inside `COMPACT_SETTLE_MS`, not `Idle`, or queue non-empty; then mark `woken` and send the resume message + parent FYI as before.

## Cost Statement (AGENTS.md)

- **Loops / timers / polls / launches:** none added (rides the existing 60 s PA watchdog round).
- **Per-seat scaling:** zero per-seat work when idle; only pending `compaction_wakes` rows are read.
- **Indexed DB reads:** non-holders (80.1% of compactions) now settle on round 1 after delivery (4 indexed reads total, 0 on rounds 2–6) instead of ~13 reads across 6 rounds; holders (19.9%) read holdings/claims during settle rounds (+2 indexed reads/min for up to 5 min). Fleet-wide net reduction in DB reads and ~5 min faster `holds-nothing` settlement.

## Verification

- New unit test `a_seat_holding_nothing_settles_immediately_after_delivery` in `crates/daemon/src/compaction_wake_tests.rs` (verifies immediate `holds-nothing` inside `COMPACT_SETTLE_MS` even while `Working` with a queued message, while confirming undelivered compacts do NOT settle prematurely).
- Existing suites: `compaction_wake`, `idle_nudge`, `memory_turn_http`, `cargo fmt --all --check`, and `clippy --locked --workspace --all-targets -- -D warnings`.
