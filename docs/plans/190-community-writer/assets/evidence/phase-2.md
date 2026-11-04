# Plan 190 phase 2 evidence: the writer on a private daemon

Phase `ph-ebe5`, written by pij-brisk-marten (coder), 2026-11-04. Script [`private-run.sh`](private-run.sh), transcript [`private-run.txt`](private-run.txt). Home and worktree paths in the transcript are shortened to `~` and `<worktree>`.

The script runs a private daemon: temp `HOME` and state dir, a private `-L` tmux server, port 17901, scratch repos. It never touches the machine daemon, `~/.pij-rs` or the live governance branch. The binary is a debug `pij-rs` built from this branch after merging phase 1 (b8196da plus the packet-pairing check). ddocs is the real one, 0.1.0, found on PATH at daemon start.

## What the run shows

| Claim | Transcript |
|---|---|
| AC3: `directory add auditor` with flags writes, validates, builds and commits; the envelope has the sha and path; the commit carries `Pij-Seat` and `Pij-Project` | `exit=0`, commit `community: add role auditor` with 7 files |
| AC2 and C9: the first write brings the three `pij/*` schema copies in the same commit; later writes leave them alone | the auditor commit lists `.dd/schemas/pij/{memory,output,role}/schema.json`; the batch commit does not |
| AC4: the prime's unstaged `a.md`, staged `b.md` and untracked `c.md` are unchanged after every write and refusal | `git status --short` before and after |
| AC6: the eight fixtures land as one commit; the same eight with `reviewer` missing its scope commit nothing | `community: import 8 roles` (32 files); the spoiled batch refuses `E-RS-DDOCS-INVALID` naming `roles/reviewer/role.dd.json`, status unchanged |
| AC5 and AC3 refusals | `E-RS-DIRECTORY-EXISTS` (auditor again), `E-RS-DDOCS-INVALID` (no scope, ddocs' own words), `E-RS-GOV-FOREIGN-EDIT` (an untracked `pm-002/role.dd.json`), `E-RS-GOV-ABSENT` with the bootstrap step (a repo with no governance worktree) |
| Every committed role is valid | `ddocs validate --depth 0` is `ok` for all nine |
| AC10: ping carries `pid` and `ddocs` | every reading shows the one pid (29514) and the resolved ddocs |

## Cost (bp-0005)

All readings are from one daemon process (pid 29514), so each pair compares.

| Window | Total launches | `git.community` | `ddocs.community` |
|---|---|---|---|
| At rest, 60 s (rest-1 to rest-2) | 0 | 0 | 0 |
| One `directory add` | 9 | 7 | 2 |
| Batch refused at its 7th document | 11 | 4 | 7 |
| Eight-role batch | 23 | 7 | 16 |

- **Per add:** 7 git (3 to find the worktree, then `status`, `add`, `commit --only`, `rev-parse`) and 2 ddocs (`validate`, `build`). Each ddocs launch is a node process: `ddocs` is a node script, started by absolute path, and its `#!/usr/bin/env node` exec's node in the same process.
- **Per batch:** git is paid once per batch (7). ddocs is paid per document (validate and build, 2 per role), because ddocs validates and builds one path per call. The plan's estimate of "the same launches once per batch" holds for git, not for ddocs. A refused batch stops at the first invalid document.
- **At rest:** nothing. There is no timer, poll or sweep; every launch is caused by a request.
- Launches go through `pij_core::program::launch` and record as `git.community` or `ddocs.community`. `crates/testkit/tests/spawn_sites.rs` passes and fails when either record is removed (checked by deleting the `git.community` record: the scan names `crates/daemon/src/community/git.rs::git`).

The other operations in the readings (`ps.*`, `tmux.*`) are the daemon's existing seat machinery, reacting once to the scratch seat's registration, and do not move at rest.

## Gates

- `just rust-check` locally: pass, 1840 tests in 117 binaries, 0 failed, 0 warnings (fmt, clippy `-D warnings`, workspace tests).
- The TS half of `harness checks` (local-paths, typecheck, lockfile, lint, vitest including `generation-skill-coverage.test.ts`) and smoke: not run locally. This worktree has no `node_modules`, and linking or installing them is out of bounds here (pm-001). They run in CI, in the check (22) and check (24) jobs, on the PR head; the PR records them as "CI on <head>". `copilot-native` passed locally. `~/pi-hacking/pij/node_modules/.bin/biome check` is clean on the changed files its config includes.
- Flake, not caused by this change: `pij-daemon --test lifecycle_http` `lifecycle_unknown_flags_and_forged_actor_are_not_silently_accepted` failed once under full-suite load (1 failure in the first `cargo test -p pij-daemon` run), then passed 3 of 3 alone, and passed in the full `just rust-check`. Recorded as D-190-1 in `docs/difficulties.md`: if it fails again it is deleted under the flaky-test rule.
- With the main checkout's binaries, from this worktree (no `node_modules` here): `~/pi-hacking/pij/node_modules/.bin/biome check .` passes (217 files, no fixes; one info about `biome.json`'s schema version, which this PR does not change); `tsx harness/scripts/local-path-check.ts` passes ("no user-specific absolute home paths in operational files"); `tsx harness/scripts/lockfile-allowlist.ts` passes (223 sources, all allowlisted).

## Review fixes (#114, pij-still-termite, verdict at b28d815)

- **F001:** ddocs exits 0 with only a warning when a link target is missing, so the writer now checks a `source: brief` packet itself. Before anything is written, the brief must be a readable file inside the governance worktree: not absolute, no `..`, no symlink out, not a directory. Otherwise the write refuses `E-RS-ROLE-BRIEF-MISSING`, naming the role and the brief. The reviewer's tests are kept. The real-ddocs existing-brief control skips by name when ddocs is absent (CI). A fake-validator twin covers missing, `../`, absolute, symlink-out and directory briefs, and checks HEAD, index and status after each. Removing the check fails both missing-brief tests.
- **F002:** each PATH entry is made absolute against the daemon's start directory before the search, so a relative (`tools`, `./tools`) or empty entry yields an absolute kept path. An execution-level test launches the found program with the governance worktree as cwd. Removing the absolutizing fails it.
- Scoped local gates after the fixes: `cargo fmt --all --check` and `cargo clippy --locked -p pij-daemon --all-targets -D warnings` are clean. `cargo test --locked -p pij-daemon` gives 933 passed and 1 failed: `http::tests::a_live_idle_claude_frame_under_a_long_stale_working_is_refused`, a test this plan did not touch, which then passed 3 of 3 alone, a second load flake. `spawn_sites` passes 4 of 4. The full-workspace `just rust-check` was not re-run: free disk fell to 23 GB, under the 25 GB floor, so building stopped. CI runs rust-test and rust-lint on the new head.
- **S1 (pij-wary-lynx, security):** git status does not flag a clean, tracked symlink, so the writer now checks every destination itself. That covers the role, its rendered `.dd.md` and the schema copies. Each destination and every ancestor inside the worktree is checked with lstat, before anything is written and again just before each write. The rollback snapshot reads and restores only regular files. (An `O_NOFOLLOW` open was tried, but `nix` in pij-daemon is a crate-graph change that `arch_drift` refuses without a ratified allow-list edit, so it was dropped. The remaining window is a local process swapping a link in between the lstat and the write, which happens under the per-worktree lock.) A symlink refuses `E-RS-GOV-SYMLINK`, naming the link. Regressions: a dangling tracked link at the destination, a symlinked ancestor directory, and links at the rendered sibling, a schema copy and the `.dd` directory. Each refuses with HEAD, status and the link unchanged and nothing outside the root, and in the last three the role document is never written. Removing the check fails all three tests.
