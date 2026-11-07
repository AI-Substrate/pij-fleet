# pij-yielding-bison — PA presence nudges and large-seat FYIs (plan 217)

**magicWand:** a warm shared cargo target per stream. On a machine at load 50,
a cold `check -p pij-daemon --tests` in a fresh worktree target took 44 minutes
and each test-binary relink about 20 more, almost all of it rebuilding the same
dependencies every other worktree already has. The serialized `shared-build`
lock makes sharing safe; a warm per-stream default would make it fast.

**What shipped:**
- Four checks inside the existing 60 s PA watchdog round, per scope (a prime and
  the live seats in its repository, plus each PA's scope): a missing-PA wake to
  the prime at 20 active minutes, a dormant-PA wake at 30 turn-less minutes, an
  unresponsive-PA wake to the prime 10 minutes after that, and a held FYI (never
  a wake) to the prime when a fleet seat goes over 600k tokens.
- Pure trackers and thresholds in `pij_core::watchdog` (`ScopePresence`,
  `pa_verdicts`, `large_seats`); claims in `presence_claims` (migration 0038),
  released only on positive evidence, so a restart can delay a message but never
  repeat one.
- Copilot seats report their size: `session.usage_info` is ephemeral (never in
  `events.jsonl`), so the extension sends it on the `/v1/activity` post it
  already makes, and a session-status decorator fills the unknown fact.
- `repo()` now remembers a folder git cannot place for 10 minutes instead of
  launching `git` for it every round.

**difficulties:**
- `last_event_at` is the seat's newest spine event of any kind, including the
  watchdog's own audits and delivery receipts on that seat, so it cannot mean
  "took a turn". The dormant clock uses the latest `seat.activity` instead.
- The Copilot size was not in the transcript at all, only in an ephemeral SDK
  event; found by reading the SDK's `index.d.ts` in `~/.copilot/pkg`.
- Existing tests assert the exact list of `watchdog.round` audits, so new
  verdicts on the same kind broke two unrelated tests. They now filter with
  `edge_rounds`.
- No `node_modules` in the worktree (C25): the extension tests need only Node
  built-ins and ran directly with `node --test`; biome came from another
  checkout's binary at the lockfile's exact version.
