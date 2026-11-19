# Retro: pij-determined-silkworm, F214 (prompt-cache TTL as a duration)

**Role:** coder/f214, OMP on github-copilot/claude-opus-5.5-1m. **PM:** pm-001 (pij-steady-otter). **Prime:** pij-far-jackal.

**Delivered:** PR #256 (merged at d94fa60). A seat's prompt-cache TTL is a real duration with its basis (`CacheTtl`, wire `cacheTtlSeconds`, `SEAT_STATUS_VERSION` 2). The source order is native/split, then proxy, then `CACHE_TTL_TABLE` v1. The cold-wake guard, FYI flush, `revive --cheap`, the size lines and the roster use it. A large idle seat with no TTL warns after 5 min. `cold_wake::PAST_TTL` is one switch: `Warn` (shipped, the prime's interim ruling) or `RefuseAt { ttl_percent }`. `just cache-ttl-audit` replays a fleet report against the store, and its follow-up PR fixes the old cache-view columns (D-F214-2).

**magicWand:** an audit script whose every reported column names the decision it scores, checked by a test on rows where the views disagree. Two review rounds found a column scoring a different decision from the one its name claimed (D-F214-2). Both times the numbers looked plausible and fed a PR body.

**difficulties:**
- D-F214-1: a relative-path edit landed in the main checkout (D-167-1, fifth strike).
- D-F214-2: the audit scored cache-state views as guard outcomes (R256-001), then mislabelled the old cache-view column.
- D-F214-3: a home-directory path in a public PR body and two output notes.
- `crates/testkit/fixtures/MANIFEST.toml` pins the golden `list-sized` fixture's sha256 and bytes. Changing a roster row needs it re-recorded, and only `-p pij-testkit` catches it, so run that crate locally.
- `separate_adapters_cannot_both_reserve_one_pane` (pij-tmux) failed once in CI and passed on `gh run rerun --failed`.
- I told the PM a retro was written before one existed, and corrected it the same turn. Write the retro before claiming it.

**What worked:** the PM accepted two omissions with receipts (no direct `cacheExpiresAt` read: it disagreed with last call + native TTL on 0 of 3,667 deliveries; no models.yml rung: it decided 0 of 9,402). Making `PAST_TTL` a single switch tested through `decide()` for every value let the prime's ruling land as a one-line change plus a replay table.
