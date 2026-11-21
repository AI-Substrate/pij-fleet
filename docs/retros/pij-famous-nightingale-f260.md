# Retro: pij-famous-nightingale, F260 (lapsed-receiver park)

**Role:** coder/f260. **PM:** pm-001 (pij-steady-otter). Branch `f260-lapsed-receiver-park` from main 6c94ffed.

**Shipped:** a body sent to a native Copilot seat whose receiver lease has lapsed is now held, not dropped. It is parked in the existing `jobs` row with a deadline in `not_before`, returned to the queue on the heartbeat that ends the told lapse, and delivered once through the ordinary claim. If the hold ends first, the sender is told. The OMP `pij_send` tool printed `queued` for every receipt; it now prints the real outcome. That was the incident's lie.

**Verified:**
- 7 new tests (1 store, 5 daemon, 1 TS `it.each` with four cases). Every new guard was mutation-checked: 8 mutants, all killed.
- The 127 daemon tests in the `native_tests`, `park_notice`, `pa_watchdog` and `double_seat` filters pass. CI is green at the final head. Locally, `harness checks` passed every stage but one in each of six runs. That stage was a different load-sensitive test each time, none of which this change touches: `daemon_stop`, `claude_inbound` or `statusline_e2e` tmux fixtures, or the `rust-runtime.spawn` deadline test. Each passes alone.

**difficulty (harness):** `harness checks` could not exit 0 on a loaded fleet machine. Six full runs (about 10–60 min each) each failed one different timing- or tmux-sensitive test, and each of those tests passed alone. `stretch:` a `harness checks --rerun-failed-once` that re-runs only a failed test binary once and reports both runs, so a flake reads as a flake and not as a red gate.

**magicWand:** `just shared-build -- … --then …` stops at the first failing command. A mutant batch spanning two crates therefore proves only the first crate's kill and needs a second build. A `--keep-going` (or `--no-fail-fast` across `--then`) would halve the mutation-check builds on a shared target where every build waits for the lock.

**difficulties:**
- The shared target's disk floor (25 GiB) stopped every cargo run mid-task while the volume sat at 24.6–24.8 GiB free. Held for a prime ruling, which kept the floor; Jordan freed disk.
- Three exploratory "where is the refusal" reads went to code paths that were already mostly right (`park_native_receiver`, `recover_native_delivery`). The existing manual-recovery path was 80% of the carrier, and finding that early is what kept this migration-free.
- The `edit` tool aborts a multi-file batch when one file's anchor is stale, so applying a three-file mutant batch took three tries. One file per call is the reliable shape.
