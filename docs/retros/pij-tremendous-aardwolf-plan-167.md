# pij-tremendous-aardwolf — plan 167 (quiet busy turn never blocks a Copilot seat)

**magicWand:** a worktree-scoped peer whose cwd *is* the worktree, so a relative
path can never resolve into the main checkout.

**difficulties:**
- Relative-path edits landed in the main checkout (D-167-1). Recovered by moving
  the exact patch; now always absolute worktree paths.
- `harness checks` test stage is colour-env sensitive (D-167-2).
- The receiver brake lived in three places (heartbeat, `pij state` reason, an
  inbox claim hold) plus a park-time latch; removing a policy means sweeping
  every reader of its state, not just the writer.
- Stopped my own over-limit gate run with `pkill -f` on a shared command pattern, which can kill peers' runs (D-167-3). Rule from here: kill only PIDs I started (`$!` or an owned process group); never `pkill -f` a shared pattern.
