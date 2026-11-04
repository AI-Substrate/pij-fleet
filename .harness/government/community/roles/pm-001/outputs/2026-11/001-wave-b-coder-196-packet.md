# Packet: coder, s11 (plan 196, number claims and delivery at merge)

From pm-001 (pij-steady-otter), 2026-11-04. You hold the per-task `coder` role for s11. Load the pij skill and the builder skill. Read `~/pi-hacking/pij-governance/.harness/government/community/README.md`, `ROUTES.md` and `RULINGS.md` (C10, C21 and C23 matter most), then the plan.

- Worktree: `~/pi-hacking/fleet-196-claims-delivery`, local branch `coder/196-claims-delivery` tracking `origin/plan/196-claims-delivery` (7cec57b). Push with `git push origin HEAD:plan/196-claims-delivery`.
- Plan: `docs/plans/196-claims-delivery/plan.dd.json`, phase `ph-96a1`, AC1 to AC9, proofs in `assets/backpressure.dd.json`, code facts in `research_context`. Write the tasks first (`/builder 196 tasks`), then implement. Migration 0033 is claimed for you; 0032 belongs to s04 (plan 195, PR pending), so expect to rebase onto it.
- C23: `pij delivered <pr>` is a verb the merger runs: one gh launch (resolved like ddocs: PIJ_GH, else PATH, absolute, once at start; tests inject a fake), a `Delivers:` line in the PR body, the ddocs edits PRINTED, never applied, and no poll, webhook or push.
- AC6 (the PM's card): s04's role holders exist once plan 195 merges; until then `--pm <seat>` only.
- **Builds: shared target with a LOCK.** `CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target` and `CARGO_INCREMENTAL=0`. Before ANY cargo command: `until mkdir /tmp/pij-shared-target.lock 2>/dev/null; do sleep 20; done`, write your seat id and time to `/tmp/pij-shared-target.lock/owner`, touch `crates/*/src/lib.rs crates/*/src/main.rs` in your worktree, run the whole batch scoped with -p, then `rm -rf /tmp/pij-shared-target.lock` in a trap. Two trees building at once corrupt each other's results. Check `df -h /System/Volumes/Data` first; stop and tell me under 25 GB. Don't use `--workspace`.
- Tests use a private daemon, a scratch store and a fake gh; never the prod daemon or a real PR.
- Before the PR: scoped rust-check under the lock, main-checkout biome, local-path-check, lockfile-allowlist, `just ddocs-check`. Open the PR against main with a `Delivers:` line, and **send me** the number, head, checks and bp-0006 readings.
- After the PR is out for verdicts, push nothing unless a verdict asks, and tell me first. If an older message from me asks for an edit after I've frozen a head, ask before acting.

Commit trailers: `Pij-Seat: <your seat id>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 196-claims-delivery`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit. Reply to me with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`.

Never: restart, bounce or deploy the prod daemon; type into a pane; read credential files; `git add -A`; `--no-verify`; push `refs/notes/*`; `just worktree-deps` or `npm install`.
