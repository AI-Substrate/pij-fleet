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

## Proof

The signals are implemented in the daemon's pane observer
(`crates/daemon/src/pane_observer.rs`) over the tmux adapter (`crates/tmux`);
their tests run with `cargo test -p pij-daemon -p pij-tmux`.
