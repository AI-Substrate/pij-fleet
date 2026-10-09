# pij-prior-python — PA watchdog (PAs only, fleet digest, quiet brake)

**magicWand:** `harness checks` that cannot be red on `main`. This run inherited
two red sensors from `main` (a skill line budget and three stale skill-check
strings left behind by plan 166's `link` port), and every branch pays to fix
them again until one does.

**What shipped:** the rs daemon nudges seats with role `pa`, and nobody else,
with a fleet digest. The digest covers every seat in the prime's repository
across all worktrees: context, idle, cache, ❄ wake price, declared state,
Needs-a-look flags and anomalies. A quiet fleet sends nothing. The canonical
PA brief is `skills/pij/references/prime/pa.md`.

**Credit:** the PA brief's rules come from interviews with `pij-future-cicada`
(unasphere; its PA `pij-gross-cahir`) and `pij-different-smelt` (adventure;
its PA `pij-swift-carl`), and from the eldenring o-prime `pij-hungry-lambert`'s
original `pa-compaction.md`. Every rule traced to a Jordan ruling or an
incident, not to drift.

**difficulties:**
- Scoping a prime's fleet by its folder misses its worktrees, which are siblings
  (`adventure-main` vs `adventure-groundcover`). Scope by `git --git-common-dir`.
- The core scheduler excluded `waiting` seats by an old ruling, and a PA rests
  in `waiting`. A PA-only watchdog must ignore the PA's own declared state.
- An unexcluded PA row in the quiet fingerprint would make every answered nudge
  cause the next. Excluded, and a mutation test proves it.
- A test fixture named temp dirs by clock nanos. macOS clock readings only
  change every microsecond, so parallel tests collided inside one `git init`
  (D-pa-1).
