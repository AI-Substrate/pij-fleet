# pij-mixed-moose — realign the pij skill with pij-rs (plan 175)

**magicWand:** a link/anchor check and a "named phrase must be absent" check
that run in `harness checks` over `skills/`. Of the 140 findings, about 30 were a
dead pointer or a retired word that a grep would have caught the day it went
stale: `link` called unported, `E-SELF`, `--layout window`, `#446`,
`WAITING_FOR_BUILD_CONFIG`, a governance path that no longer exists. The plan's
link checker (`assets/scripts/linkcheck.py`) works and has a proven control, but
it lives in a plan folder because the scope was frozen. Promoting it to a test is
the next encode.

**What shipped:**
- Two-sided verb gate. The TS test now also checks shim → skill, leaves included.
  A new Rust test reads the built `pij-rs --help`. Both were red on the bb9f0b1
  skill for the right reason, and each has a mutation receipt.
- 140 survey rows dispositioned in `assets/findings.md` (115 FIXED, 22 RULED,
  1 REJECTED, 0 DEFERRED, 2 UNVERIFIABLE kept with reasons). The pair route is
  rewritten to builder seats (R1). The skill is 2,491 lines (was 2,656).
- pij-fleet#58 filed for the code half of the status-stale finding.

**difficulties:**
- **A one-directional reverse check would have been green on main.** The plan
  asked for RED first. Every RS_ROUTE_TABLE top-level verb was already in the
  table; the drift was in the *leaves* (`bg emit`, `inbox check/register`). The
  check enforces leaves, which is the honest red. Lesson: before writing a
  red-first test, run the cheapest form of it against main and see which
  direction actually drifted.
- **The survey was right about a fact and wrong about its edge.** "Revive never
  checks a tombstoned seat" read true from the survey's summary. revive.rs shows
  the conversation-live check still runs on that path; only the seat's own
  process and pane go unchecked. My first draft copied the over-claim into two
  files. Re-reading the code before writing caught it, which is why the method
  says re-verify.
- `node_modules/.bin/ddocs` is absent in this worktree, though the builder recipe
  says to use it. I used the ambient `ddocs` 0.1.0 instead. `stretch:` have
  `just worktree-deps` link it, or have the recipe name the fallback.
- zsh doesn't word-split `set -- $var`, so one batched `ddocs set` loop wrote
  nothing (it errored loudly, so no harm). Use `for a b c in …` in zsh.
- The original `findings.md` id scheme had to come from the reports' table
  rows, and section C of core.md duplicates later rows. The ledger records both
  rather than merging them, so the 140 count matches the plan.
