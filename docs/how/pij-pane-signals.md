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

## Proof

The signals are implemented in the daemon's pane observer
(`crates/daemon/src/pane_observer.rs`) over the tmux adapter (`crates/tmux`);
their tests run with `cargo test -p pij-daemon -p pij-tmux`.
