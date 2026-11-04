# Packet: reviewer, PR #114 (s02, plan 190, community writer)

From pm-001 (pij-steady-otter), 2026-11-04. You hold the per-PR `reviewer` role for PR #114. Your standing packet is `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md`; follow its reading order. Read rulings C5, C7, C8, C9, C10 and C13 in `RULINGS.md`.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/114, branch `plan/190-community-writer`, **head d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed**. Worktree (detached, read-only): `~/pi-hacking/fleet-review-114`. Frozen during review; if the head ever moves, I'll tell you and give you a new worktree. Never move this one.
- Plan: `docs/plans/190-community-writer/plan.dd.json`, AC1 to AC11; proofs in `assets/backpressure.dd.json`; evidence in `assets/evidence/` (phase-1.md, phase-2.md, private-run.txt). 70 files, about 6.8k lines: the pij/role, output and memory schemas; a Rust governance-root resolver; the community writer; `pij directory add` and `--batch`; migration 0031 and the store pointer; ddocs resolution, refusals and doctor; the daemon pid on ping; docs and PRD/roadmap state flips.
- Check, at minimum:
  1. **AC4, checkout safety (C8)**, the riskiest part. The writer writes into the standing governance worktree, runs `git add -- <its paths>` then `git commit --only -- <its paths>`, and stages nothing else. Prove in a scratch repo: an unstaged edit, a staged edit and an untracked file in that worktree all survive exactly; the next ordinary commit doesn't revert pij's write; a path pij is about to write that already carries a foreign edit refuses by name and writes nothing; a held index.lock is retried briefly, then refused. Mutation-check: weaken one guard and confirm a test fails.
  2. **AC6, batch all-or-nothing**: one invalid document commits none. Also the standing_packet pairing (source inline needs text; source brief needs brief).
  3. **AC7**: the pointer rebuild from the branch head matches row for row; migration 0031 is additive and numbered right.
  4. **AC5, AC10**: ddocs from PIJ_DDOCS, else PATH, resolved once (C10); named refusals; doctor reports it; ping has data.pid.
  5. **AC8, cost**: every new launch goes through pij_core::program::launch by an absolute path, is recorded with the spawn counter, and is covered by crates/testkit/tests/spawn_sites.rs (that file isn't in the diff, so confirm it picks the new sites up and would fail on an unrecorded one). Nothing on a timer. Readings: add 9, batch-of-8 23, at rest 0.
  6. **AC9 and AC11**: PRD 001 and roadmap 001 flips match their done_when; req-03 reworded to C8; roadmap s02 `complete`; req-26 per C13. P1 to P10 where they apply (tagged-union returns, injected side effects, no global state).
- Not in scope: security (pij-wary-lynx) and cost verdicts (pij-frugal-wombat).
- Tooling: Rust builds scoped with `-p` (for example `cargo test -p pij-daemon community`, `cargo test -p pij-store`, `cargo test -p pij-testkit --test spawn_sites`). Check `df -h /System/Volumes/Data` first and stop under 25 GB. Use a private daemon or scratch repos only, never the prod daemon or the live governance branch. No node_modules; TS is covered by CI (green on this head).

## Output (do not skip)

1. Verdict file `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-114-verdict.md`, plus one line in `roles/reviewer/outputs/LOG.md`.
2. `gh pr comment 114 --body-file <file>`, starting `reviewer: pass|pass-with-notes|block (head d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed)`. Give each finding a severity and a concrete fix.
3. **Send me the report**: `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. **Your job is not done until this send has gone.**

Never commit to the governance branch; never push to the PR branch.
