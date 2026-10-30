# pij-staged-deadlock: pij's own staged text held a seat forever (plan 184, pij-fleet#86)

**magicWand:** a launch-latency sensor in `harness checks`: p50/p99 of
`/usr/bin/true` on the machine, next to `just spawn-rate`. The 1 s stage bound
was set against a launch costing a few milliseconds. On this fleet machine a
launch has a p99 of about 300 ms at ordinary load. Nothing measured that, so a
bound sized for one machine quietly became a deadlock trigger on another.

**What shipped:**
- **Own-text recognition.** A composer holding exactly
  `staged_leftover(text)` for the text this attempt would stage is pij's own;
  anything else is a human draft. The matcher is in core (`is_staged_leftover`)
  and allows only rendering freedom. The new `TmuxPort::clear_staged_leftover`
  verb refuses on any difference, inside an owned transaction. A command whose
  leftover was cleared fails `undelivered:staged-unsent`; a body is retried into
  the blank composer.
- **Reads before reserves.** The gate looks through the control client first. A
  held delivery waits at zero launches (red on main: 20 attempts launched 20
  reserves and 20 restores).
- **Bounded, surfaced holds.**
  - Composer-busy holds back off 1 s doubling to a 60 s cap.
  - At 12 attempts a command fails `undelivered:hold-limit`, which unblocks the
    seat's FIFO.
  - At the same limit a body tells its sender once (`delivery.hold-limit` →
    `hold-notice-<job>`) and stays queued.
- **The stage bound itself.** It was 1 s with a 2 s watchdog; it is now 4 s with
  an 8 s watchdog, justified by measured launch latency (diagnosis.md).

**difficulties:**
- **The first diagnosis was too narrow** (high). Both log hits predated the
  plan-182 daemon, so "load" looked like the whole cause. The prime's 08:03
  retry on walrus failed again at ordinary load. What answered it was timing
  `/usr/bin/true`: the tail sits in process launch, not in tmux. **Lesson:**
  when a bound trips, measure the latency distribution of what it bounds. A
  single "today a launch takes 0.00 s" reading (p50) hid a p99 of 300 ms.
- **The fake queue is not FIFO under re-pend** (medium). `FakeQueue` let six
  messages past a re-pended control head. The real sqlite queue does not, so
  the "head blocks the queue" test was green-for-the-wrong-reason on main until
  it moved to `SqliteQueue`. `stretch:` make `FakeQueue::claim` honour
  per-serial FIFO like the store.
- **Real-tmux fixtures were sized to the old watchdog** (low). Three adapter
  tests ran `sleep 5` in the pane. With an 8 s watchdog the pane exited first,
  and the failures looked unrelated. A fixture lifetime should derive from the
  constant it waits on.
- **No local `ddocs`** in a fresh worktree, and installing is forbidden here. I
  used the global `~/.npm-global/bin/ddocs`. `stretch:` have `harness plan
  validate` carry the writer, or allow the global binary.
- **A residual the screen cannot resolve.** A line break typed straight after
  an existing space renders exactly like a soft wrap, so the matcher accepts
  it. It changes no characters. Width-aware matching (pane width → wrap
  points) would close it.

**Side findings (not fixed, out of scope):** the machine daemon was at 200% CPU
and 8 GB RSS, with `background reconciliation failed … owner … is gone` at about
10 Hz (most of a 500 MB `daemon.log`). A one-chunk stage could be two launches
instead of six (opener, body and closer in one buffer); that would cut its cost
by two thirds and narrow the tail.

## Review round (#91, reviewer pij-review-pr91)

**magicWand (round 2):** a shared "hold clock" abstraction in core. B1 was a
policy that counted retries when it meant time. `attempt` looked like the
natural counter, and every other retry path moves it. A typed "held since"
with its own reasons would have made the wrong counter unwritable.

**difficulties:**
- **I counted the wrong thing** (high, my blocker). The hold limit counted
  `attempt`, so `human-typing` burned through it in one grace, and the walrus
  deploy would have failed its head as "human-typing after 12 attempts". Both
  were false. Fixed by holding time per job in the worker since the first
  counted hold. Typing never counts and waits out the gate's `retry_after`.
- **My "told once" test passed on a fake that resets `attempt`** (medium).
  The dedupe key was `msg_id`, but the fact carried `messageId`. That is the
  same cross-seam literal trap as plan 114. The test now runs on the real
  queue across passes, and a mutant that drops `msg_id` turns it red.
- **No test told the look from the owned read apart** (medium). Every fixture
  served one capture to both. A witness now scripts a different second capture.
- **"Changes no characters" was false** (medium). The screen cannot show a
  person's whitespace at a line end, and N backspaces then stop short. The fix
  re-reads after the clear and fails by name if the composer is not blank. It
  also checks the caret and judges a row break as a wrap only where the next
  word could not have fitted. A residual remains and is documented: invisible
  whitespace at a line end other than the caret's row is deleted with pij's
  text, and the delivery then fails by name.
- **The disk filled mid-fix** (environment). No shell could start, so I could
  not even report it over pij. The prime freed space. Keep builds scoped
  (`-p`).
- **A merged sibling (plan 186) moved Copilot controls onto
  `delivery-control:`.** The rebase was clean, and its live case (held
  `unrecognized`) is why `unrecognized` now counts toward the limits.
