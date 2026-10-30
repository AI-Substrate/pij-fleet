# Recheck: PR #95 at c1f8314

Reviewer: pij-review-pr95. Diff `c0258a2..c1f8314`. Read-only; worktree clean afterwards.

**Verdict: merge. 0 blockers, 0 should-fix, 1 nit.**

## 1. S1: fixed
My wedge probe (real SqliteQueue and DrainWorker, lease 1 s, drain dies holding the compact claim) against the new head:
```
native body claim after restart, pane kept : Some("b-1")
native body claim after restart, paneless  : Some("b-1")    (was None at c0258a2)
```
The drain now lists `delivery-control:<seat>` for every Copilot seat (`worker.rs:130-134`). A paneless claim is released with backoff (`worker.rs:236`), so the serial key is freed and nothing is typed. The new test `an_abandoned_copilot_control_claim_never_wedges_native_bodies` passes, and M8 turns it red.

## 2. S3: closes the window, launch-free, adds a small serial delay
- **Window.** Blips measured 2–4 ms, so a read 250 ms later lands outside any one blip. The control is typed only if both reads land inside separate blips: about 2e-4 × 2e-4 per pass, which is negligible. What remains is a genuinely new turn starting between the second read and Enter (the gate's capture and stage take tens of ms). That gap exists with any brake and is not the blip. `copilot_compact_waits_out_an_idle_blip_inside_a_turn` passes, and M7 (single read) turns it red.
- **Launch-free.** The second read is `registry.get`, a SQL read, plus a tokio sleep. No tmux and no process launch.
- **Serial drain: yes, a bounded stall.** `drain_once` handles claims one after another (`worker.rs:151-170`), so the sleep delays every later claim in the same pass by 250 ms. It only sleeps when the first read is already `Idle` (busy seats return at once), and a pass makes at most one claim per kind. The worst case is 250 ms × the number of Copilot seats with a due compact that read idle, on a 5 s tick. That is acceptable. While it sleeps, the control row is `running`, so that seat's native bodies wait 250 ms too, which is intended.

## 3. S2, S4, N1: correct
- **S2.** `00-routing.md:29` (C1) and `:83` (C3) now say Copilot `compact` is supported and that `new`/`reload`/paneless refuse. The coder also fixed `prime/orchestrator.md:57` and `routes/pair.md:45`, which I had missed. `peer.md` now says "settled idle".
- **S4.** `copilot-native-extension.md` and `pij-rs-api.md` now state both directions, and C3 and `pair.md` say to let the compact land before the next packet.
- **N1.** The draft test now uses the canary's 1.0.105 frame, `❯ hello world`, with the caret at (13, 3). This matches my earlier probe (`Recognized("hello world")`).

## 4. Breakage: none found
- `cargo test -p pij-daemon --lib` → 640 passed (the worker and delivery suites, including the 250 ms path).
- `--test copilot_compact` → 5/5; `--test control_commands` → 8/8.
- CI on `c1f8314`: check(22), check(24), rust-lint, rust-test all pass.

## Nit
- C3 and the docs say to "check `pij state`" to see that the compact landed. As far as I know, the card shows a compact only once it has been held (a `delivery_deferrals` row), not while it waits unclaimed or after it has been typed. The firmer signals are the `delivery.outcome` event for the compact's msg_id, or the context dropping in the footer. Wording only; I didn't verify the card's full contents.

## Not run
I didn't re-run M7 and M8 myself; I read their receipts and confirmed the tests they name. I also did not run TS locally (CI ran it), a live Copilot canary, or `harness checks`.
