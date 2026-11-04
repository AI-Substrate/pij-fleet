# Packet: coder, s04 (plan 195, phase 2)

From pm-001 (pij-steady-otter), 2026-11-04. pij-brisk-marten, you hold the per-task `coder` role for s04. Rulings: C1, C6, C8, C9, C10, C13, C16, C22 in RULINGS.md.

- Worktree: reuse `~/pi-hacking/fleet-190-community-writer` (keeps your 12 GB target). Check it's clean, then `git switch -c plan/195-assign-lineage origin/plan/195-assign-lineage` (df591de). Push to that branch.
- Plan: `docs/plans/195-assign-lineage/plan.dd.json`, phase `ph-95b2`, AC2 to AC12, proofs in `assets/backpressure.dd.json`, code facts in `research_context`. Write the tasks first (`/builder 195 tasks`), then implement.
- Phase 1 (pij/holders and pij/external schemas, AC1) is docs-system's, on branch `plan/195-schemas`, and comes after heron's current job. Start with the parts that don't parse holder documents: migration 0032 (claimed for you), the close hook (AC4), seat_roles per C1 (AC3), the C6 hook (AC5), window rename via the control client (AC6), help (AC10). Merge `origin/plan/195-schemas` when I say it has landed.
- Open question 2: heron's dogfood import may call for a writer fix (an existing role folder with memory/INDEX.md refuses as "exists"). I'll tell you if it lands in this plan; don't fix it pre-emptively.
- Cost: AC11 and bp-0007. Every launch goes through pij_core::program::launch by absolute path with the spawn counter; the view and the rename launch nothing.
- Disk: check `df -h /System/Volumes/Data` before building; stop and tell me under 25 GB. No node_modules: TS proof is CI on the exact head; biome via ~/pi-hacking/pij/node_modules/.bin/.
- Before the PR: `just rust-check` (scoped where you can), the main-checkout biome, local-path-check and lockfile-allowlist, and `just ddocs-check`. Then open the PR against main and **send me** the number, head, checks and bp-0007 readings.
- **Pushes**: after the PR is out for verdicts, push nothing unless a verdict asks, and tell me first. If you get an older message from me asking for an edit after I've frozen a head, ask before acting on it.

Commit trailers: `Pij-Seat: pij-brisk-marten`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 195-assign-lineage`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit.
