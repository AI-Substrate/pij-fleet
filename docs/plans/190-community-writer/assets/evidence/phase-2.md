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
