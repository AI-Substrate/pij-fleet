# pij-willing-coyote — PA watchdog on the idle edge, and on usage-limit reset (plan 172)

**magicWand:** a rulings ledger the packet links to, so a rule that replaces an
earlier one says so in place. R1/R2 → option A → the idle edge arrived as three
messages. The second one was a full replacement, and nothing in the plan file
showed that the first had been superseded.

**What shipped (supersedes PR #38):**
- PAs are nudged on the fleet's **idle edge**: once, when nobody else in scope is
  working any more. A new episode needs someone to work again first. There is no
  cadence, no fingerprint and no change detection.
- A fleet idle on a **usage limit** gets no edge nudge. Instead it gets one wake
  per `(scope, resetsAt)` at the reset plus 120s, to the PA or else the prime.
  The source is Claude Code's structured `quotaLimits` entry, never the display
  text.
- R4 is kept from #38: bounded reads with the #35 index fix (F2), and silent
  opt-in no-ops (F3).

**difficulties:**
- The Unisphere SDK already folds Claude limit notices (`LimitSeen`), but its
  `resets_at` is the display phrase ("3:10pm (Australia/Brisbane)"). The ruling
  forbids parsing that phrase, so pij reads `quotaLimits.resetsAt` from the
  transcript tail itself (`pij_harnesses::ClaudeUsageLimits`). `stretch:` have
  Unisphere carry `resetsAt` as an epoch so there is one reader.
- No tz database crate is in the workspace. The local zone name at an instant
  comes from `date` (BSD `-r`, GNU `-d @`), which is DST-correct, and tests pin
  `TZ=Australia/Sydney` and `Australia/Brisbane` per child process. A process-wide
  `TZ` set would be unsound in a multi-threaded test binary.
- A builtin rule asks for `parking_lot` over `std::sync::Mutex`. `parking_lot` is
  not a workspace dependency, and the repo's convention (pij-unisphere) is
  `std::sync::Mutex` with `.expect`, so that convention was kept rather than
  adding a dependency under scope freeze.
