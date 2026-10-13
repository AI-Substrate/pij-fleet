# pij-fresh-cattle — plan 171 (a self-named seat never takes a retired id; pij_spawn never fails silently, pij-fleet#44)

**magicWand:** an agent `edit`/`write` tool that refuses a relative path when the
seat's packet names a worktree, or that resolves relative paths against the packet's
worktree. The packet said "absolute paths only", and the first two edits still landed
in main (D-171-1, third strike of D-167-1).

**difficulties:**
- D-171-1: relative-path edits resolved into the main checkout. The two-file patch
  was moved with `git apply` and reverse-applied in main. One hunk had also landed
  inside `internal()`, and only `cargo test` caught it.
- D-171-2: proving the self-named path live needs a known OMP session id, because
  the name seed is `pi\0<session>`. Worked recipe: run OMP once with
  `--session-dir`, read the id from the session file, seed a retired row (different
  harness and session) in the isolated store, then relaunch with `--resume <id>`.
  The first candidate from the seed function equalled the first run's name, and the
  resumed run registered as the second candidate.
- Load sat at 43–55 for about 25 minutes before the first test run (C12). Most of
  the session's wall time was that wait.
- `tmux set-environment -g PATH` does not reach a spawned pane. The extension
  passes its own environment, so a stub `omp` has to be on the spawning
  process's PATH.

**Noteworthy:**
- Inferred from reading `registration.rs`, not exercised: a non-native
  registration whose id belongs to a **live** seat of a different process
  appears to rebind that row instead of refusing. The extension avoids this only
  because it skips names in the live roster, so a read-then-register race could
  take a live name. Out of scope for #44; verify it, then file its own issue.
- With no Copilot changes, Claude/Copilot/Codex spawns now also wait up to 20s for a
  bound seat. A slow-binding harness reports `spawned, NOT registered` honestly. Raise
  `PIJ_SPAWN_REGISTER_TIMEOUT_MS` if that is noisy.
