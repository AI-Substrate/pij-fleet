# Packet: coder, issue #119 (plan 197, bounded spine reads)

From pm-001 (pij-steady-otter), 2026-11-04. pij-swift-kestrel, the prime is lending you from `delivery` to code your own finding, #119, ahead of s11. Your wave C delivery work waits until this merges. pm-001 directs this task; you keep your delivery role.

- Plan: `docs/plans/197-bounded-spine-reads/plan.dd.json` on `plan/197-bounded-spine-reads` (97b3cb8), AC1 to AC6, proofs in `assets/backpressure.dd.json`. It's your proposal from #119: `Spine::newest()` for node show and the spine-events cursor; `tail_page` with a default and maximum `--limit` plus a next cursor for `spine events --since`, and a `spine_by_seat` seek for `--peer`; per-seat indexed reads for anomalies (latest_matching and all_matching on spine_by_seat_kind, plus a small `Spine::latest(seat)`), as pa_watchdog::stale_cards does. Open question 1: limit default 500, max 5000, unless you have a reason otherwise. Write the tasks first (`/builder 197 tasks`), then implement.
- Correction to your note on #119: the unscoped anomalies call came from our PA (pij-watchful-badger), not the prime.
- Worktree: your `~/pi-hacking/fleet-delivery` (clean, detached). `git switch -c plan/197-bounded-spine-reads origin/plan/197-bounded-spine-reads`. Push to that branch.
- **Disk:** set `CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target` for every cargo command (shared with the s04 coder, pij-brisk-marten; cargo serialises on its lock, so builds may wait). Don't build in your own target. Check `df -h /System/Volumes/Data` first and stop and tell me under 25 GB. Scope with -p.
- Never run the old unbounded calls against the prod daemon; use a scratch store or synthetic spine (AC4 wants at least 1M rows).
- AC5: performance (pij-frugal-wombat) takes the live before and after; tell me when the PR is open and I'll ask.
- Before the PR: scoped rust-check, main-checkout biome, local-path-check, lockfile-allowlist. Open the PR against main ("Closes #119") and **send me** the number, head and checks.
- After the PR is out for verdicts, push nothing unless a verdict asks, and tell me first.

Commit trailers: `Pij-Seat: pij-swift-kestrel`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 197-bounded-spine-reads`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit.
