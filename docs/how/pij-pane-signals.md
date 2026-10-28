# pij pane signals

The pij daemon maintains three live signals for every tmux pane from one
`pipe-pane -O` output tap and one `list-panes -a` query per tick.

## Signals

- **Busy** is a read-only byte-density signal. Sustained output in a rolling
  one-second window marks the pane busy; 1.5 seconds of silence returns it to
  idle. Busy never delays or blocks delivery.
- **User typing** is a delivery hold. TUI cursor reports
  (`ESC[row;columnH`) reveal composer length relative to the learned idle
  caret. A non-empty composer holds sends; Enter resets the caret and releases
  them. Sixty seconds without a keystroke also releases the hold.
- **Connected** comes from the all-server pane id set. New live ids attach a
  tap; absent ids and `pane_dead=1` retire the signal and detach the tap.

## Delivery contract

The existing daemon `SendBuffer` owns the queue. Messages received while
`userTyping` is true remain durable and unread in the target inbox, are
deduplicated in the in-memory FIFO, and flush in arrival order when the hold
releases. A busy pane with an empty composer receives messages immediately.

The daemon does not use `capture-pane`, process CPU, or a keylogger for these
signals. Existing `capture-pane` readiness and heartbeat behaviour is separate.

## Composer recognition (capture-pane)

Typed delivery first finds the composer in a pane capture
(`crates/harnesses/src/composer.rs`). An empty composer submits; a draft holds
as composer-busy; a layout it cannot find defers as `unrecognized` and retries.

Codex is recognised by the footer under its `›` prompt:
`<model> <effort> · <cwd>`, and since Codex 0.164
`<model> <effort> · <cwd> · <session title>` (live:
`GPT-6.1-Sol high · ~/github/novels · Adopt seat and wait`). A usage segment
(`85% context left`) also marks a footer. One parser, `parse_codex_footer`,
serves composer recognition, readiness and the observed model and cwd: the
model is the first word, the cwd is the first segment after it that is a path,
and the title is ignored, even when it contains `·` or looks like a path.
Reading only the text after the last `·` took the title for the cwd, so Codex
0.164 seats stayed `unrecognized` until plan 177.

The first segment must be exactly `<model> <effort>`. Codex also prints a quota
warning, `⚠ 5h limit: 49% left · resets at 12:33 AM on 27 Oct · /status`, with
the rest of a footer's shape; it read as model `⚠`, cwd `/status` until plan
181. Hint lines (`← for agents · ? for shortcuts … ⚠ 2 warnings · f2 to view`)
are not footers either. `← for agents` marks a TUI hosted by the shared Codex
app-server; how pij binds and identifies Codex seats is in
[pij-rs-api](pij-rs-api.md#codex-seats-plan-181).

## Staged submits, their leftovers, and held deliveries (plan 184)

Typed delivery and every Claude control command (`pij send --command compact`)
go through one **owned transaction** (`InteractionGate::submit_with_owned_input`,
`crates/harnesses/src/interaction.rs`). It reserves the pane (input disabled,
token held), re-reads the composer, pastes the body, and presses Enter only into
a composer that was blank.

**Reads before reserves.** Reserving costs two launches (reserve, then restore).
So the gate first looks through the control-mode client, which launches nothing.
A composer that is plainly not submittable defers without reserving. The look
is a brake, not the decision: everything it lets through is judged again under
ownership. A held delivery therefore waits at zero launches
(`crates/daemon/tests/held_delivery_launches.rs`).

**Stage bound and watchdog.** Staging is bounded by `SUBMIT_STAGE_LIMIT` (4 s),
and a tmux-side watchdog restores input after `SUBMIT_RECOVERY_LIMIT_SECS`
(8 s) if the daemon never does (`crates/tmux/src/lib.rs`). The bound is about
launch latency, not tmux or the pane's program: a one-chunk body is three
owned pastes at two launches each. On the fleet machine, launches measured p50
7 ms, p90 140 ms, p99 280–390 ms and max 553 ms under ordinary load. That
holds even for `/usr/bin/true`. The old bounds of 1 s and 2 s tripped on that
tail. The watchdog outlasts the stage bound plus an abort's three launches, so
it can no longer take the token mid-abort (`exit 75`).

**The abort path.** A stage that fails after pasting (the bound, a tmux error,
a lost token) pastes `STAGED_SUBMIT_RECOVERY` after the body and releases the
pane. That leaves `<staged text>`, a newline, and
`[pij: delivery staged but not submitted; inspect the staged text and press Ctrl-C to clear it]`.
The job is retried as `submit-error`.

**Own-text recognition.** On the next attempt, the composer may hold exactly
pij's own leftover: `staged_leftover(text)` for the text this attempt would
stage (`crates/core/src/ports.rs`). If so, and the typing grace has passed, the
gate clears it inside a fresh owned transaction. Nothing is submitted.

- The tmux verb `clear_staged_leftover` itself refuses unless the token still
  owns the pane, nothing is staged, and the observed composer is exactly the
  leftover. It then sends one BSpace per character.
- "Exactly" allows only the composer's own rendering. A row break must stand
  where the leftover has a space or newline (a soft wrap, or the notice's
  line). Continuation rows may lose up to two columns of indent, and row
  padding is ignored.
- Any other difference is a human draft and is never touched: a typed or
  deleted character, a doubled space, or another text's leftover.
- One edit cannot be seen on screen: a line break typed straight after an
  existing space renders exactly like a soft wrap, so it is cleared with the
  leftover. It changes no characters.

What happens next depends on the delivery:

- a **control command** fails by name: `undelivered:staged-unsent`. A
  compaction requested earlier may no longer be wanted, so the sender decides
  whether to resend;
- a **body** is retried at once (`leftover-cleared`) and staged cleanly into
  the now-blank composer.

**Holds back off.** A `composer-busy` hold (a parked draft, or a leftover not
yet recognized) waits on a person, so it backs off: 1 s, 2 s, 4 s … capped at
60 s (`HOLD_BACKOFF_CAP`). Typing (`human-typing`) already waits out its grace
inside the gate. The other holds (`tap-unowned`, `unrecognized`) keep their
existing schedules.

**Holds are bounded and surfaced.** At 12 attempts (`HOLD_LIMIT_ATTEMPTS`,
about seven minutes of composer-busy holding):

- a **control command** fails `undelivered:hold-limit`, naming the hold reason.
  The seat's later deliveries are then claimed in order: delivery is FIFO per
  seat, so a held command used to block every message behind it;
- a **body** stays queued and its sender is told once. The worker commits a
  sender-addressed `delivery.hold-limit` fact
  (`{messageId,jobId,recipient,reason,attempts}`), and the park-notice follower
  sends `[pij] message … is still held (<reason>) after N attempts…` under the
  id `hold-notice-<job_id>`.

Both terminal command failures commit a sender-addressed `delivery.parked`
fact (transport `pty`), so the sender gets the ordinary park notice.

Proof: `crates/daemon/src/pointer/worker/tests/plan184.rs`, the gate tests in
`crates/harnesses/src/interaction.rs`, the matcher tests in
`crates/core/src/ports.rs`, the real-tmux tests in `crates/tmux/tests/adapter.rs`
(`clear_staged_leftover_*`, `staging_at_the_measured_launch_tail_still_submits`,
`slow_launches_strand_a_short_command_with_the_recovery_notice`), and the park
notice tests in `crates/daemon/src/park_notice.rs`. The diagnosis is in
[plan 184](../plans/184-staged-submit-deadlock/assets/evidence/diagnosis.md).

## Proof

The signals are implemented in the daemon's pane observer
(`crates/daemon/src/pane_observer.rs`) over the tmux adapter (`crates/tmux`);
their tests run with `cargo test -p pij-daemon -p pij-tmux`.
