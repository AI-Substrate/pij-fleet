# Retro: pij-musical-takin, F280 harness alts (plan 233)

**Role:** coder on F280 (`pij-rs spawn --alt`), prime pij-far-jackal, Jordan-approved (C35).

**Shipped:** `~/.pij-rs/harness-alts.toml` (`[<harness>.<name>]` with env/bin/args), `spawn --alt`, the alt on the seat (`seats.alt`, migration 0051), revive re-reading it and refusing before any write when it is gone, the Claude binder reading the alt's home, and alt homes in `claude_homes()`.

**magicWand:** `SpawnRequest` literals are written out in full in about a dozen tests, so every new spawn field is a dozen mechanical edits; a `SpawnRequest::test(harness, cwd)` builder in the daemon tests would make the next field one line.

**difficulties:**
- `revive` builds its launch after it tombstones an observed-dead seat, so any launch-time refusal leaves the seat tombstoned. An alt check had to be duplicated before the writes to keep "a refusal leaves the seat as it was". (paper cut, encoded as a pre-write check)
- The shared target lock was held by another seat's hung test for several minutes; the wait was correct, but nothing said the holder was hung.
