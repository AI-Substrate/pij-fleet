# Plan 202 evidence: the routing table

Phase `ph-00ca`, written by pij-local-warbler (coder), 2026-11-06. Script [`private-run.sh`](private-run.sh), transcript [`private-run.txt`](private-run.txt), with the worktree path shortened to `<worktree>` and the host name to `<machine>`.

## Gates (this worktree)

All cargo runs through `just shared-build` into the lane target, scoped with `-p`.

| Gate | Result |
|---|---|
| `fmt --check`, `clippy --all-targets -D warnings` on pij-daemon, pij-core, pij-cli | clean |
| `test -p pij-daemon -p pij-core -p pij-cli -p pij-testkit` | all green except `review::review_reassign_within_one_second_keeps_both_holdings` (see below); `spawn_sites.rs` green with no new site |
| new tests | `community::routes` (6 unit tests); `directory_http.rs`: `role_changes_regenerate_routes_in_their_commit_and_tell_the_prime_once`, `routes_on_demand_and_write_only_when_stale`, `assign_compact_and_revive_each_send_a_holder_the_table_once` |
| `ddocs-check` (main checkout's tsx, no node_modules here per C25) | 7 documents validate clean, `.dd.md` current |
| `ddocs validate` roadmap, PRD, blueprint 001 | 0 errors, 0 warnings |
| main checkout's `biome check .` | clean |
| `local-path-check`, `lockfile-allowlist` | clean |
| TS (`just typecheck`, vitest) | CI on the exact head (no TS changed) |

**The same-second test is the known F003 flake, not this change.** `review_reassign_within_one_second_keeps_both_holdings` (from #122, fixed by plan 206) needs an assign, release and reassign inside one wall-clock second. It passes alone (420-760 ms) and fails under parallel load. An A/B with this plan's assign-path FYI switched off by a temporary env guard failed the same way (`off`: 1 of 2 runs failed at 833 ms; `on`: 1 of 2), so the extra FYI does not cause it. The guard was removed before commit.

`a_live_idle_claude_frame_under_a_long_stale_working_is_refused` failed once in the full lib run under load and passed alone and in the following full run; it does not touch routes.

## Cost (bp-0004)

Private daemon from this branch (debug `pij-rs`, real ddocs), temp HOME and state, private `-L` tmux server, port 17922, scratch repos. Launch deltas from `pij-rs --json ping` (`data.spawns.by_op`), same daemon pid throughout:

| Step | Launches | By op |
|---|---|---|
| at rest, 60 s apart | 0 | |
| `directory add delivery` (rewrites the hand-written ROUTES.md in the same commit) | 9 | git 7, ddocs 2 (unchanged from k-1) |
| `directory add storage` (overlap, ROUTES.md rewritten) | 9 | git 7, ddocs 2 |
| `directory routes` | 0 | |
| `directory routes --write` on a current file | 0 | checked in-process first |
| `directory routes --write` on a stale file | 7 | git 7 (root 3, status, add, commit, rev-parse), no ddocs: ROUTES.md is not a document |
| `directory assign` (plus the holder's routes FYI) | 10 | git 7, ddocs 2, tmux rename 1 (unchanged from plan 195) |
| `directory release` | 9 | git 7, ddocs 2 (unchanged) |

Held FYIs after the run: the prime got two (one per add that changed rows; the stale `--write` changed no rows and sent none), the holder one (the assign); release sent nothing. Compact and revive share the assign's delivery function (`http/routes.rs::deliver`): a few store reads, an in-process read of the role documents and one held FYI, no launch; they are proven by the HTTP test, not the private run, because a scratch seat cannot pass the control identity check without a real harness process.

No timer, poll or new launch site: regeneration rides the role write's existing commit, and every delivery is triggered by a verb.

**Live daemon at rest, before deploy** (pid 93273, read-only pings): 949 at 00:35:36Z, 1025 at 00:36:36Z, so 76 launches in 60 s (1.27/s). `just spawn-rate --seconds 20`: machine 319.9/s (other lanes' cargo builds and agent shells), daemon children 0.4/s. The after-deploy reading is owed once this merges and the daemon is bounced; this plan changes no at-rest path.

## Known limits (for the reviewers)

- **Copilot and `/compact`.** The compact FYI is held, and controls never carry FYIs, so for OMP (which latches delivery while compacting) it lands in the first turn after the compact. For Copilot the FYI is claimed by `onUserPromptSubmitted`; if that hook fires for a typed `/compact`, the table would be claimed into the pre-compact context. Not proven either way here.
- **Compactions the daemon never sees** (a harness compacting on its own, a person typing `/compact`) send nothing.
- **Scope edits.** No verb edits a role's scope yet. Any later verb that writes role documents through `DirectoryService::write` (scope, retire, merge) regenerates and notifies for free; one that writes through another path must call `routes::regenerate` itself.
