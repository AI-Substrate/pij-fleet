# pij-pa-gaps: PA watchdog gaps (plan 178)

**magicWand:** a `watchdog.nudge` replay fixture. Each opt-in rule (interval,
working, declared wait, cold, answered `ready`) is a separate condition in two
places, `optin_due` in core and the round in the daemon. Each new test had to
build a whole store and drive a minute-by-minute fake clock to reach one row of
that table. A table-driven fixture of seat states → nudge or skip, run against
the real round, would have made the three rules one test each and none of them
boilerplate.

**What shipped:**
- #68: a quota rejection stops its scope only until its wake is due
  (`usage_limit::limit_current`, `resetsAt` + 120 s). The reset wake still reads
  every rejection, so it fires once as before. One current stop still pauses the
  whole scope (R1).
- Opt-in nudges ask the send guard's verdict first. A cold seat is skipped, and
  the skip is audited on `watchdog.nudge` with the guard's facts. A seat that answers
  with `ready` stays quiet until a round sees it working. Both are brakes.
- `pij watchdog status` lists live PAs and opt-ins only. There is no `retired_*`
  field: nothing consumes one, and pij-rs-api.md's `{pas, optins}` stays true.
- pa.md R1–R5 plus the cold-wake note; pa-watchdog.md documents all of it. A core
  test reads pa.md with `include_str!` and pins it to the nudge's card command
  (mutation-proved).

**difficulties:**
- The plan's ac-0007 named `bp-0004` as its pressure, but the backpressure doc
  has only bp-0001..0003. `ddocs validate` failed (E430) the moment an execution log
  existed. Repointed to bp-0003 (the gates row). Severity low; a `harness plan
  validate` at plan-commit time would have caught it, but it validates only when the
  log exists. `stretch:` validate link targets even when the log is absent.
- Risk rk-0001 says a missing `resetsAt` "keeps today's behaviour (stopped)". Today
  a record without a numeric `resetsAt` is not a rejection at all, so today's
  behaviour is *not stopped*. Behaviour is unchanged; the docs now say which.
- `http::native_tests::native_spawn_wait_resolves_prebound_child_resuming_saved_session`
  failed once in the full pij-daemon run: a 2 s registration timeout under
  full-suite load. It passed 3/3 alone. It is unrelated to this change. A
  wall-clock bound in a test is a flake source; `stretch:` inject the wait.
- The Mac hard-reset mid-run; the revive kept the conversation, and the
  worktree was clean at the plan commit, so nothing was lost. The packet's C12
  `-j 1`/nice gating was then retired by Jordan mid-run; the first red build
  was restarted at full parallelism.
- `zsh` does not word-split `set -- $pair`, so a ddocs loop silently addressed
  `tk-0001 1`. Re-ran under `bash`. Papercut, first strike.
