# pij-crooked-thunderbluff — double-seat warning, comparison B (plan 174)

**magicWand:** a `just cargo-c12 <args>` recipe that applies the load rules
itself: `nice -n 19 timeout`, `-j 1`, the worktree's own `CARGO_TARGET_DIR`,
and a wait while the 1-minute load is over 40. Every packet restates C12 as
prose, and prose did not stop me. I chained two builds in one shell command,
and the second started at a load of 45.8 because nothing checked again between
them. A recipe would have waited.

**What shipped (PR #57, comparison implementation B):**
- `crates/daemon/src/http/double_seat.rs` holds the detection once:
  `conversation_live_as_seat` plus the Claude session-record witness. Revive,
  adopt and pane resolution all ask it.
- `E-RS-CONVERSATION-LIVE` is defined once, in `pij_core::double_seat`. Two
  paths raise it:
  - adopt of a conversation that is live as a seat in another process;
  - every pane-resolved verb from a pane running a second copy: the identity
    ladder `resolve_seat`, and `bound_seat` for the hooks.
- The live seat gets one notice per (seat, stray pid+start). The spine event is
  written first and is also the dedupe record. Delivery goes through a
  `send_or_hold` extracted from park_notice, so cold-wake rules apply.
- The SessionStart and UserPromptSubmit hooks warn the human and the model. The
  status line shows a red `⚠ double` and stops healing that pane.

**difficulties:**
- **One helper, two prior semantics.** Before this plan, revive and adopt had
  different notions of a "live elsewhere" seat:
  - revive: process `Active` only, and tombstoned rows count;
  - adopt: process "not observed gone", and live rows only.

  The guide asked for one helper and also for revive to stay unchanged, and
  both cannot hold literally. I chose "live row, process not observed gone".
  For revive that has two effects:
  - **More conservative:** a failed probe now refuses instead of launching a
    copy. That direction is a brake.
  - **Less conservative:** a tombstoned row no longer counts by itself. For
    Claude, its running process is still found through its session record.

  The docs say both. A reviewer should look at this first.
- **Pre-existing hazard (not fixed, out of scope).** `router_with_config` builds
  its `RegistrationService` with `pij_harnesses::claude_homes()`, which is the
  operator's real home, not `Services.claude_homes`. So any HTTP test that
  registers a Claude seat runs `ensure_claude_inbound_accept` against the real
  `~/.claude`. That includes
  `adopt_upgrades_unknown_claude_consent_but_preserves_explicit_refusal`. My
  tests avoid it by seeding the live seat and never registering a Claude seat.
  The one workspace gate run still executes those older tests. `stretch:` have
  the registration service take its homes from `Services`.
- **`harness plan pr-body` vs CI.** It refuses while any AC is open, and ac-0008
  needs green CI on the exact head. So the PR opens with an interim body. Closing
  ac-0008 then needs one more commit, which needs one more CI run. `stretch:`
  allow `pr-body --allow-open <ac>`, which renders the open row as pending.
- **Red for a literal pin.** A test that pins a script literal to a new Rust
  constant cannot compile on the base tree. Its red receipt is therefore the
  constant present with the bb9f0b1 scripts. It is recorded that way, not as a
  red on bb9f0b1.
- Papercut, my own: a `set -- $pair` loop in the default zsh shell does not split
  words. I re-ran it under `bash`.

**Encodable improvement:** the `cargo-c12` recipe above. Second choice: a
`RegistrationService` that reads `Services.claude_homes`, so test hermeticity
comes from the fixture rather than from each test's discipline.

## Fix round (review of cae54cd, 2026-10-24)

**magicWand (second):** each reviewer finding should arrive with a
"red on the reviewed head" requirement already attached. That framing made this
round mechanical. The reviewed head's own source acts as the mutation for every
regression finding: swap the file, see the test fail for the reason the finding
names, swap it back.

**difficulties:**
- **I unified two semantics the plan said to leave alone.** The guide asked for
  "one helper, revive calls it". The non-goals said revive's guard must not
  change. Those two conflict. I resolved it myself and documented the change,
  when I should have asked the prime at the ack. The window for raising a
  contract conflict is widest before anything is built. Revive is now restored
  verbatim; only adopt and pane resolution share the helper.
- **Store-live is not running (F002).** The identity ladder already skipped an
  owner row whose process was gone. The hooks' `bound_seat` did not, so a dead
  owner row silently swallowed `activity`, which wrote `working` to a corpse,
  and `fyi-claim`. Two resolvers for one question drift. `stretch:` make
  `bound_seat` call the ladder's pane step.
- **A lock with no witness.** The notice lock was correct, but no test could
  fail without it. On a current-thread runtime a race only shows up if the test
  forces it to overlap. A gate spine that holds the first dedupe read until a
  second reader arrives makes that deterministic.
- **LOW findings not fixed.** One: the notice runs under one daemon-wide lock;
  per-seat locking needs a keyed map. Two: older tests reach the real
  `~/.claude` through `RegistrationService`. Pointing it at `Services.claude_homes`
  changes what those tests observe about consent. Neither fix is trivial, so
  both stay as stretch items.
