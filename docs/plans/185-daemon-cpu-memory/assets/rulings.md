# Plan 185 rulings (prime, 2026-10-29)

- **R1 Read-only profiling.** On the machine daemon use only tools that don't pause or change it: `sample`, `vmmap`, `heap`, `footprint`, `ps`, `pij-rs --json ping`, read-only sqlite, `du`. Never attach lldb/dtrace in a way that stops it, and never bounce, restart or deploy it. The prime deploys and measures after merge.
- **R2 Private daemons only for experiments.** Bisecting or replaying uses a private daemon with a temp state dir on a private `-L` tmux server (`pij_testkit::tmux::PrivateTmux`), never the default tmux server.
- **R3 Diagnose first.** Send the prime the diagnosis pointer before fixing, then carry on unless the prime replies. Fix only causes the diagnosis shows.
- **R4 Same behaviour.** Pane signals, bg completions and deliveries must behave identically; prove it.
- **R5 Public repo.** Evidence holds pij-generated data only, never pane transcript content from any seat.
- **R6 Ship.** Commit, push the branch, open the PR (standing approval). Do not merge. `git ls-remote origin 'refs/notes/*'` must be empty after every push.
