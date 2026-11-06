# Packet: coders, the rest of roadmap 001 (06 Nov push)

From pm-001 (pij-steady-otter), 2026-11-06. Jordan wants the whole roadmap merged today so he can try **Pij Emergent Communities (PEC)** (ruling C26). Small PRs merge fast; the gate stays (reviewer, security and performance verdicts at the exact head).

## Lanes
| Lane | Coder | Plans, in order |
|---|---|---|
| A | pij-swift-kestrel | 199 s05 send to a role, then 205 s12 time limits |
| B | (new) | 201 s07 role memory, then 200 s06 outputs (once s05 merges) |
| C | (new) | 202 s08 routing table, then 203 s09 prime prompts (once s05, s06 and s07 merge) |
| D | pij-following-marsupial | 194 s03 templates, then 204 s10 community route (once s09 merges) |
| E | pij-exuberant-wildcat | 196 s11 claims, then 206 s04 follow-ups |

Each plan is on `origin/plan/<NNN>-<slug>`: short ACs, a cost row and a docs/states AC. Write the tasks first (`/builder <NNN> tasks`), then implement. Load the pij and builder skills. Read the community README, ROUTES.md and RULINGS.md in `~/pi-hacking/pij-governance/.harness/government/community/`.

## Shared seams
crates/cli wiring, crates/daemon/src/community/ and skills/pij are touched by several lanes. Keep the diffs there minimal, rebase or merge origin/main right before opening the PR, and re-run `just ddocs-check`. If you need to change something another lane owns, tell pm-001 rather than forking it.

## Builds
- Lanes A, B, C and E share one target: `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target just shared-build -- <cargo args>`, for EVERY cargo command, scoped with -p (never --workspace). Builds queue on the lock; batch your commands (`--then`).
- Lane D (marsupial) uses its own target.
- Stop and tell pm-001 under 25 GB free. No node_modules, never symlinked (C25); TS proof is CI on the exact head; biome via ~/pi-hacking/pij/node_modules/.bin/.

## PR
- Before opening: scoped rust gate through the recipe, main-checkout biome, local-path-check, lockfile-allowlist, `just ddocs-check`. Body: a `Delivers:` line plus the checks and cost readings.
- Send pm-001 the PR number, head, checks and cost readings. Then freeze: no pushes unless a verdict asks, and tell pm-001 first. If an older message from pm-001 asks for an edit after a freeze, ask before acting.
- Never use a question UI; ask pm-001 in plain text.

Trailers: `Pij-Seat: <your seat>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: <NNN-slug>`, Co-Authored-By. Stage by path only. Card report at the start and end of each unit. Reply with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`.

Never: restart, bounce or deploy the prod daemon; type into a pane; read credential files; `git add -A`; `--no-verify`; push refs/notes; `just worktree-deps` or `npm install`.
