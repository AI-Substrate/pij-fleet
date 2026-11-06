# #135 (plan 206) cost receipt, tk-0007

Seat: pij-exuberant-wildcat (coder). Date: 2026-11-06.
Binary: debug `pij-rs` built from 3621220's code (no code change since) on the fleet-194 target under `nice -n 19 taskpolicy -b`; run under `nice -n 19` only.
Script and transcript: `docs/plans/206-directory-followups/assets/evidence/private-run.{sh,txt}` on the PR branch (e60be54). A private daemon, temp HOME and state dir, a private tmux server and port, and a scratch checkout with its own prime-governance worktree. Never the machine daemon.

| Step | Launches (spawn counter delta) |
|---|---|
| rest, 60 s | 0 |
| directory add (each of 4) | 9 (ddocs.community 2, git.community 7) |
| directory edit | 9 (2 + 7) |
| directory retire | 9 (2 + 7) |
| directory merge --into | 11 (ddocs 4 for its two documents, git 7; one commit) |
| send role:<retired> (refused E-RS-DIRECTORY-RETIRED) | 0 |
| send role:ops (queued, no holder) | 0 |
| retire ops with a queued request (refused E-RS-DIRECTORY-ROLE-QUEUED, listing it) | 0 |
| retire ops --drop-queued | 9 (2 + 7) |
| directory view | 0 |

Each verb is one writer commit, the same launches as add (merge builds two documents). Nothing runs on a timer. The queued check is one read on `role_requests_queued`; --drop-queued adds one store transaction and one FYI per withdrawn request, with no launch.
