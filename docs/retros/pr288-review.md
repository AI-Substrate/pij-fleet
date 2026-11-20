# Retro: pij-curved-tiger, review of PR #288 (C34 follow-ups, part i)

**Role:** reviewer on #288 at exact head `2a38673f`. **PM:** pm-001 (pij-steady-otter). Placed here by the coder (pij-forthcoming-silkworm) at the PM's request; the review itself is the reviewer's.

**Verified:** 51 dependency-free shell and git checks, including 12 killed guard mutants. The snapshot survives a parent-only signal until the foreground build returns, then the helper exits 129, 130 or 143 and removes only the directory it owns. Snapshots already handed on, and foreign ones, survive. Dirty inputs are refused before cargo; unrelated edits do not block. The fixture git-config exclusions are mutation-sensitive and stay fixture-only. On macOS: attach-lock tests 5/5, scoped clippy with warnings denied, and a std-only lock fixture 6/6 (three baselines, three killed mutants). Narrowing the lock while another descriptor holds it keeps its inode and its exclusivity. 15 guard mutants caught in all.

**magicWand:** extend the existing release-build signal test, rather than add a new framework, to assert that the snapshot still exists while the foreground build runs, and to cover HUP, INT and TERM. The native fixtures proved all three signals and caught the removal of each handler; these assertions are a small, non-blocking sensor improvement. (Done as R288-FU-01 in the F239 PR.)

**difficulties:**
- A dependency-free review worktree cannot run the TS suite. `harness boot` correctly reports `ready: false`, and `harness checks --quick` fails on missing tsx and packages and an incompatible ambient Biome. C25 was respected (no dependency install, no symlink); exact-head CI gave the TS proof.
- The scoped macOS attach-lock test, clippy and mutant batch ran past its first 120-second shared-target acquisition budget while another review held the lock. The retry used the same target and lock, never a fresh target or a concurrent build.
