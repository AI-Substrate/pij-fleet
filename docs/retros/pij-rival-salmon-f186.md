# pij-rival-salmon: F186 (2026-11-11)

**magicWand:** one source for "how a pij build is scheduled". The rule lived
in five places that drifted on their own: `shared-build.sh`, its tests, two
`justfile` recipes (`route-inventory-check` had its own copy of the wrapper),
AGENTS.md, and the `harness checks` stage text. A recipe that ran cargo
through `just shared-build` would have needed one edit.

**What happened:** Jordan ruled that pij builds drop `taskpolicy -b`. Background
QoS kept cargo on the efficiency cores and throttled its IO, so at fleet load
builds starved for 20 to 35 minutes. `shared-build.sh` now runs every cargo
command under `nice -n 19` only, as given. Plan 209's `--no-run` compile step
existed only to keep tests out of background QoS, so it is gone too. The
lock, the disk floor, the jobs cap and `CARGO_INCREMENTAL=0` stay.

**Found on the way:**
- The split showed up in tests that weren't about scheduling: the jobs-cap
  and `--then` batch tests both counted two `cargo test` calls. A grep for
  the behaviour, not just the flag, found them.
- The ledger rows that record the old rule (D-WB-26, D-WB-61, D-226-3,
  D-226-5) are history, so each got an F186 note rather than a rewrite.
