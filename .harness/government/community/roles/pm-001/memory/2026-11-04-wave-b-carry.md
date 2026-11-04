# Carry into wave B packets

tags: rulings, wave-b, packets
hook: C18 must be folded into the s03, s09 and s10 packets; req-27 carries it in PRD 001.

- Ruling C18 (Jordan, 2026-11-04): communities are self-discovered from their needs; templates are examples, not a checklist. The /pij community route and the prime's prompts must say so, and prompt the prime to spot uncovered needs and create roles for them. Fold into the s03, s09 and s10 packets. PRD 001 req-27 (heron, plan 193 PR) records it.
- C6 and C1 for s04: assign exposes a queue-drain hook for s05, and sets seat_roles for pm/pa/worker.
- C16: one tmux window per seat, named `<project>:<role>`, with the instance for many-holder roles (`pij:coder-190`, `pij:review-<pr>`).
- Retire native seats by `pij close` and then ending the process by pid (#109); `pij close` alone does not stick.
- Roadmap steps use `complete` (no `shipped` in the pij/roadmap enum). Every ddocs write needs a biome format pass on docs/project JSON.
- Community worktrees have no node_modules and may not get any: TS proof is CI-only; biome from ~/pi-hacking/pij/node_modules/.bin/.
- Owed to the difficulty ledger (next docs PR): two load/CI flakes found during #114, `http::tests::a_live_idle_claude_frame_under_a_long_stale_working_is_refused` (1 fail under full pij-daemon load, then 3/3) and crates/tmux `staged_submit_excludes_human_input_mid_body_and_before_enter` (CI teardown "server exited unexpectedly", rerun passed). Flaky-test rule: delete if it fails again.
