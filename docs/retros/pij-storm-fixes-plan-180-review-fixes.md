# pij-storm-fixes: review fixes for PR #73 (plan 180)

**magicWand:** a `tmux` test fixture crate that every Rust test reaches for by
default: a private server, a "human" client, a stall switch and a
control-client probe. This session wrote all four from scratch
(`PrivateTmux`, `human_attach`, `Resume`, `control_clients`), and the daemon
crate already has its own `LiveTmuxSession`, which is a third copy. S3 existed
because the easy path (`daemon_config(.., false)`) quietly meant "the
operator's tmux". The default should have been the private server, with the
operator's server something a test has to opt into.

**What shipped:** S1 `_pij` session (one launch: `-N -C new-session -A`, plus
`destroy-unattached` and a session-count subscription so an emptied server still
exits); S2 write-barrier text and an allow-list pin over all 167 tmux 3.6
names; S3 `Config::tmux_server` / `PIJ_TMUX_SERVER` and `PrivateTmux`; S4
server-stall test, `Arc::ptr_eq` before clearing, corrected reason; nit 4.
Details and receipts: `docs/plans/180-spawn-storm/assets/reviews/review-fixes.md`.

## difficulties

1. **The suggested stall did nothing.** The packet said to SIGSTOP the `tmux -C`
   child. tmux passes the client's stdin and stdout to the server, which then
   reads and writes our pipes itself, so a stopped client still answered (in
   125 µs). The fix was to stop the private server. Lesson: in tmux control mode
   the client process is not on the data path.
2. **S1's proof condition was half unsound, and only an experiment showed it.**
   A dedicated session fixes bare `attach`, but any attach makes its session the
   most recently active, so untargeted outside commands follow it. I stopped and
   asked one question; the prime ruled in a few minutes. Asking cost far less
   than shipping a "parity" test that is false for half of what it claims.
3. **A fix created a new leak.** A session that holds a client open also holds
   the server open. Nobody had raised this, and only a test that closed the last
   session found it.
4. **`set-option -t =_pij` fails** ("no such session") while `-t _pij` works.
   Untargeted from inside the client is simpler, and right for this client.
5. **My mutant runner lied once.** `cargo test "$2"` passed `--lib control` as
   one argument, and cargo's first failing binary hid the rest. Two "survived"
   results were really "never ran". Fixed with `--no-fail-fast` and unquoted
   arguments (green-that-lies, again).
6. **S3's red cannot be shown safely.** Reverting the fix would touch the
   operator's server. The substitute is a positive artifact: the daemon's control
   client appears on the private server.
