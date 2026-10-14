# pij-evident-dog: the PA resumes when the prime does (plan 173)

**magicWand:** a cheap "head seq" read on the `Spine` port. The brake has to ask
"was anything delivered to the prime since I armed?". The natural cursor for
that is a spine sequence number. But `matching_since` orders by `(at, seq)`, and
the port has no head read, so the arming cursor has to be the round's wall
clock. That works because receipts and rounds share `system_time_ms` in
production. The tests had to put receipts at fake-clock times to line up with
the fake rounds, though, and a skewed clock source would quietly widen or narrow
the brake window.

**What shipped:**
- A pure `PrimeActive` tracker in `pij_core::watchdog`, next to `IdleEdge`. It
  arms at the first tick after the worker idle edge where the PA and the prime
  are both idle, and wakes the PA once on the prime's next working tick.
- The brake is a `delivery.outcome` `delivered` receipt to the prime since
  arming. It disarms and keeps the cap, so the PA's own night report can't spend
  the morning wake. That was the amendment to AC3.
- `docs/how/pa-watchdog.md` describes the whole PA watchdog. The PA brief and
  the pij-rs-api.md section point to it.

**difficulties:**
- The plan's AC5 asks for a `no-prime` audit on `watchdog.round`. Its dw-0003
  says every other #52 test is unchanged, and one of those pins the round audit
  list to exactly `["nudged", "nudged"]`. Resolved by carrying `prime_active:
  "no-prime" | "watching"` on the edge's own audit instead of a separate event.
- `682c45e` vendored `.dd/schemas/builder/*.json`, which Biome's formatter
  rejects, so `just lint` failed on a tree this plan didn't author. They were
  excluded in `biome.json` the same way `docs/plans` already is. `stretch:` the
  builder vendoring step could add the exclusion itself.
- The fresh worktree had no `node_modules`, so every TS gate stage failed with
  exit 127 until `npm ci` ran. `harness checks` doesn't say so. `stretch:` a
  preflight line, "no node_modules: run npm ci", in `harness checks`.
- Load hit 70-80 mid-run, which is past C12's 40 ceiling. The Rust gate waited
  on a load-polling loop. Nothing in the harness encodes that ceiling.
- Fix round (PR #53 review): `ddocs add --mint lg` mints **hex** ids (`lg-0009` →
  `lg-000a`). I assumed decimal and linked `lg-0010`. Both `ddocs set` and
  `harness plan validate` accepted the dangling `proven_by` without a word, so it
  landed in 125fdcd and was relinked in the next commit. `stretch:` validate
  should fail an address that doesn't resolve, and `add` could print the id it
  minted as its only stdout line.
- The reviewer's C12 wrapper gates launch on the max of the 1/5/15-minute
  load averages. At a sustained load of ~45 it blocked for its whole 20-minute
  deadline. Gating on the 1-minute average (and pausing on it) worked. A pause
  during a timing-sensitive test can still contaminate the run, so a paused test
  run is evidence only once it has been re-run clean.
