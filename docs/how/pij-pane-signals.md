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

Tap files are bounded on disk: a live sink is emptied once consumed past
1 MiB, and each boot sweeps the sinks of panes the server no longer lists.
See [daemon-cost.md](daemon-cost.md#tap-files-on-disk).

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

Typed delivery and every pane control command (`pij send --command compact` to
a Claude seat, or to a Copilot seat on its `delivery-control:<seat>` kind since
plan 186) go through one **owned transaction**
(`InteractionGate::submit_with_owned_input`, `crates/harnesses/src/interaction.rs`).
It reserves the pane (input disabled, token held), re-reads the composer, pastes
the body, and presses Enter only into a composer that was blank.

**Reads before reserves.** Reserving costs two launches (reserve, then restore).
So the gate first looks through the control-mode client, which launches nothing.
A composer that is plainly not submittable defers without reserving. The look
is a brake, not the decision: everything it lets through is judged again on a
read taken under ownership, and only that read decides. A held delivery
therefore waits at zero launches
(`crates/daemon/tests/held_delivery_launches.rs`).

**Stage bound and watchdog.** Staging is bounded by `SUBMIT_STAGE_LIMIT` (4 s),
and a tmux-side watchdog restores input after `SUBMIT_RECOVERY_LIMIT_SECS`
(8 s) if the daemon never does (`crates/tmux/src/lib.rs`).

- The bound is about launch latency, not tmux or the pane's program: a
  one-chunk body is three owned pastes at two launches each.
- On the fleet machine, launches measured p50 7 ms, p90 140 ms, p99 280–390 ms
  and max 553 ms under ordinary load, even for `/usr/bin/true`. The old bounds
  of 1 s and 2 s tripped on that tail.
- The watchdog outlasts the stage bound plus an abort's three launches, so it
  can no longer take the token mid-abort (`exit 75`).
- While input is disabled, keys a person types into the pane are **dropped,
  not buffered**. A stage normally takes tens of milliseconds. In the tail, up
  to 4 s of keystrokes can be lost, or 8 s if the daemon dies mid-transaction.
- A body near the 8 KiB frame limit is nine chunks, about 22 launches. At the
  measured tail it can still pass 4 s and abort, which the leftover rules below
  handle.

**The abort path.** A stage that fails after pasting (the bound, a tmux error,
a lost token):

1. closes the bracketed paste if it opened one;
2. pastes `STAGED_SUBMIT_RECOVERY` as its own paste;
3. releases the pane.

That leaves exactly the text the stage got into the composer, a newline, and
`[pij: delivery staged but not submitted; inspect the staged text and press Ctrl-C to clear it]`.
The adapter records the text it pasted on the transaction (`StagedSubmit::pasted`),
and the gate keeps that record per pane. The job is retried as `submit-error`.

**Own-text recognition.** On a later attempt for the same text, the composer
may hold exactly pij's own leftover. That is one of:

- the leftover the gate recorded at the abort, which may be a prefix of the
  text;
- the full-text form `staged_leftover(text)` (`crates/core/src/ports.rs`). A
  complete stage leaves this, and a restart cannot forget it. A prefix left
  before a restart is not recognised; it ends at the hold limits below.

pij acts only if all of these hold, on the read taken under ownership:

- the typing grace has passed (a fresh daemon treats its first sight as an
  edit, so it waits one grace);
- the composer matches exactly;
- the caret sits right after the leftover.

pij then clears it inside that ownership and **re-reads the composer**:

- **Blank:** a control command fails by name, `undelivered:staged-unsent`. A
  compaction requested earlier may no longer be wanted, so the sender decides
  whether to resend. A body is retried at once (`leftover-cleared`) into the
  now-blank composer.
- **Not blank:** no further key is sent. The pane is released and the delivery
  fails by name: `undelivered:staged-unsent: … composer not cleared; left for
  the operator`.

The tmux verb `clear_staged_leftover` refuses on its own unless all of these
hold:

- the token still owns the pane;
- nothing is staged;
- the observed composer is exactly the recorded leftover.

It then sends one BSpace per character of the leftover and waits 250 ms for
the pane to render.

"Exactly" allows only what the composer's rendering does where the screen
proves it:

- a row break stands where the leftover has a newline, or a space that a soft
  wrap consumed. A break counts as a wrap only if the next word could not have
  fitted on the row, judged against the widest row **shown**. The composer's
  real width is unknown here, so this is a lower bound: it rejects a break
  well short of the widest row, not every typed newline (see below);
- continuation rows lose up to two columns of indent;
- line-end spaces, which a capture never shows, are ignored.

A typed or deleted character, a doubled space, a line break well short of the
widest row, or another text's leftover is a human draft and is never touched.

**What a capture cannot show.** Whitespace a person adds can still pass as
pij's own in two ways:

- spaces at the end of a line other than the caret's row, which tmux trims;
- a newline typed in the second half of the widest line. The typed break also
  shortens that row, so the width rule, measured against the widest row shown,
  still takes it for a soft wrap (review recheck of #91).

The BSpace count is exact for pij's text, so it stops short of the start of the
composer. The re-read shows what is left, the delivery fails by name, and the
composer is left for the operator. The person's added whitespace, which sits
inside the deleted span, is deleted with pij's text. Nothing else is deleted:
every visible character before the caret must equal pij's leftover and the
caret must sit at its end, so a typed or deleted non-whitespace character is
never cleared. This needs a deliberate whitespace edit inside pij's own notice,
then a full grace of stillness.

**Holds back off.** Each job's holds are tracked in the worker for this daemon
run, from its first counted hold. They are never counted from `attempt`, which
every other retry also moves; a restart starts the clock again, which only
lengthens a hold.

| hold | counts toward the limits | retry |
|---|---|---|
| `composer-busy` (a draft, or a leftover not recognized) | yes | 1 s, 2 s, 4 s … capped at 60 s (`HOLD_BACKOFF_CAP`) |
| `unrecognized` (a layout pij cannot read; also how a two-line leftover reads in Copilot's one-row box) | yes | existing schedule |
| `human-typing` | never: it ends with its grace | after the gate's remaining grace (`retry_after`) |
| `tap-unowned` | no: the observer attaches a tap on its next tick | existing schedule |

**Holds are bounded and surfaced**, by how long a job has been held:

- **7 minutes** (`HOLD_LIMIT_MS`): a **control command** fails
  `undelivered:hold-limit`, naming the hold reason, the count and the
  duration. The seat's later deliveries are then claimed in order: delivery is
  FIFO per kind, so a held command used to block everything behind it.
- **7 minutes**: a **body** stays queued and its sender is told once. The
  worker commits a sender-addressed `delivery.hold-limit` fact
  (`{msg_id,messageId,jobId,recipient,reason,holds}`). Its `msg_id` is the key
  the worker checks, so the fact is written once even across passes and
  restarts. The park-notice follower then sends `[pij] message … is still held
  (<reason>) after N composer holds…` under the id `hold-notice-<job_id>`.
- **20 minutes** (`HOLD_FAIL_BODY_MS`): a **body** fails
  `undelivered:hold-limit` too. pij cannot tell a leftover it does not
  recognise from a human draft, so a body must not wait forever behind either.

Every terminal failure above commits a sender-addressed `delivery.parked` fact
(transport `pty`), so the sender gets the ordinary park notice.

Proof:
- `crates/daemon/src/pointer/worker/tests/plan184.rs`, including the walrus
  deploy modelled on its real state and both Copilot control-kind cases;
- the gate tests in `crates/harnesses/src/interaction.rs`;
- the matcher tests in `crates/core/src/ports.rs`;
- the real-tmux tests in `crates/tmux/tests/adapter.rs`;
- the park-notice tests in `crates/daemon/src/park_notice.rs`.

The diagnosis is in
[plan 184](../plans/184-staged-submit-deadlock/assets/evidence/diagnosis.md).

## Proof

The signals are implemented in the daemon's pane observer
(`crates/daemon/src/pane_observer.rs`) over the tmux adapter (`crates/tmux`);
their tests run with `cargo test -p pij-daemon -p pij-tmux`.
