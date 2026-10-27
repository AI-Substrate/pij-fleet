# Review fixes for PR #73 (plan 180)

Fixer seat `pij-storm-fixes`, for prime `pij-far-jackal` (Jordan: "do it"). Review: `pij-review-pr73`, verdict *merge after fixes*, 0 blockers, 4 should-fix, 7 nits. Fixed here: S1 to S4 and nit 4. The other nits go to a follow-up issue the prime files. Rulings: `../rulings.md` R-PRIME-1 to R-PRIME-4 (R3 and R5 amended by excision).

Every tmux experiment and test ran on a private `tmux -L` server, killed afterwards. The operator's default server was only ever read (`list-clients`, `list-sessions`) for the rule-3 proof.

## S1: the control client gets its own session, `_pij`

- `crates/tmux/src/control.rs`: `CONTROL_SESSION`, and `start()` runs one launch, `tmux -u -N -C new-session -A -s _pij -f no-output,ignore-size,read-only 'exec sleep 2147483647'`. `-N` means no server is ever started; `-A` attaches if `_pij` exists. Before the handshake the client sets `destroy-unattached on` on `_pij` and subscribes to `#{server_sessions}`; `Framer` turns `%subscription-changed pij-sessions … : 1` into `Frame::Alone`. The client then leaves, `_pij` is destroyed, and an emptied server exits as it did before. The restart after `Alone` waits out `RESTART_BACKOFF`. `first_session()` (a `list-sessions` launch) is gone, so warm-up is now 1 launch, not 2.
- `crates/tmux/src/lib.rs` `list_panes`: never returns a pane in `_pij`. Every consumer (pane observer reconcile/tap sweep, reaper, revive, caller-pane resolution, pointer worker, interaction gate) gets panes only from `list_panes`, so none of them can see it.
- The prime's stop condition triggered: untargeted commands from outside tmux still resolve to `_pij` after each control attach, until the next keypress elsewhere (`exp3.py`). The prime ruled to ship with this residual, documented and pinned (R-PRIME-1).
- Tests (`crates/tmux/tests/control_mode.rs`): `a_bare_attach_lands_where_it_would_without_the_control_client`, `an_untargeted_outside_command_resolves_to_pij_after_the_control_attach` (residual pin), `the_control_session_is_attached_read_only_and_never_listed`, `the_server_still_exits_when_the_operator_closes_the_last_session`, `reads_never_start_a_server`. The "human" is a `tmux -C attach-session` client: same `cmd_find` path as a terminal attach, and it needs no pty.

## S2: the text names the real write barrier

- `control.rs` module doc and the `ATTACH_FLAGS` comment: the barrier is `is_read` (deny by default) plus `command_line` quoting; `read-only` only refuses keyboard input.
- `docs/how/macos-defender.md`: a "What stops it writing" bullet, plus the `_pij` and residual bullets.
- PR body: section rewritten (see the PR).
- Test `read_allowlist_is_exactly_four_reads`: all 167 tmux 3.6 command names and aliases. Exactly `capture-pane`, `display-message` (with `-p`), `list-panes` and `show-options` are reads; unknown first words and empty argv fork.

## S3: tests never touch the operator's tmux server

- `Config::tmux_server` (`crates/core/src/config.rs`), wired in `crates/daemon/src/lib.rs` (`for_server[_with_legacy_tap_root]`), and `PIJ_TMUX_SERVER` read by `pij-rs daemon` (`crates/cli/src/main.rs`).
- `pij_testkit::tmux::PrivateTmux` creates a private server with one idle session and kills it on drop.
- Pointed at it: `shipped_daemon_config.rs`, `sidecar_producer.rs` (both tests), `statusline_e2e.rs` and `claude_inbound.rs` (a spawned daemon, via the env). `governance_boot.rs` already isolates through an inherited private `TMUX`. Every other pij-cli daemon uses `FakeTmux` or `--offline`.
- Positive proof that the private server is the one in use: `shipped_daemon_config` now waits for the daemon's control client to appear on its private server.

## S4: stall test, generation check, corrected reason

- `a_stalled_control_client_is_killed_and_replaced`: SIGSTOP on the `tmux -C` process stalls nothing, because tmux hands the client's stdio to the server, which then talks to our pipes directly. So the test stops the private SERVER for 4 s. The read returns the forked answer after ≥3 s, reads reconnect, and the new client is a different pid.
- `forget(state, waited_on)` clears `state.live` only when `Arc::ptr_eq` matches the waiter's own client. Test: `a_stale_waiter_never_clears_the_replacement_client`.
- Comment at the timeout branch and R-PRIME-4: replacing the client recovers from a hung client; the queue (the timed-out waiter's sender absorbs its late reply) is what keeps replies routed.

## Nit 4

`justfile`: `local-path-check` has its comment back, and `spawn-rate`'s summary line is the one `just --list` shows.

## Red before green

| test | red against | observed red |
|---|---|---|
| `a_bare_attach_lands_where_it_would_without_the_control_client` | pre-fix code | `left: "b" right: "a"`: the control client moved the human |
| `an_untargeted_outside_command_resolves_to_pij_after_the_control_attach` | pre-fix code | `left: "a" right: "_pij"` |
| `the_control_session_is_attached_read_only_and_never_listed` | pre-fix code | failed at the `_pij` session assertion |
| `the_server_still_exits_when_the_operator_closes_the_last_session` | `_pij` without `Alone` (mutant `noalone`; also my first cut, where `set-option -t =_pij` failed) | `the server outlived its last real session: "_pij: 1 windows … (attached)"` |
| `reads_never_start_a_server` | green pre-fix (the old code checked with `list-sessions`); it guards `-N` | not separately reddened |
| `a_stalled_control_client_is_killed_and_replaced` | mutant MA | `the stalled client was kept, not replaced` (left 46940, right 46940) |
| `a_stale_waiter_never_clears_the_replacement_client` | unconditional clear (mutant `cascade`) | `a stale waiter cleared the new client` |
| `read_allowlist_is_exactly_four_reads` | `is_read` widened with `run-shell` (mutant `widen`) | failed |
| `only_the_pij_sessions_subscription_at_one_means_alone` | mutant `noalone` | failed |

## Mutants

| id | mutation | result |
|---|---|---|
| MA | timeout keeps the client | **killed** by `a_stalled_control_client_is_killed_and_replaced` |
| MB | `ATTACH_FLAGS = "no-output"` | **killed**, but only by a flag-string assertion in `the_control_session_is_attached_read_only_and_never_listed`. Behaviourally still not load-bearing: `ignore-size` changed no window size on tmux 3.6 (review `size.py`), and `read-only` only blocks keyboard input, which this client never sends. |
| cascade | `forget` clears unconditionally | killed |
| widen | `is_read` admits `run-shell` | killed |
| noalone | `Alone` never framed | killed (unit and integration) |

## Rule-3 proof (default server)

`cargo test -p pij-cli --no-fail-fast`: exit 0, 132 tests, no skips. Default server, read only: `list-clients` before = after (empty), `list-sessions` before = after (12 sessions, byte-identical). Sampled every 0.5 s during the run: 0 control-mode clients, 0 `_pij` sessions.

## Not run, and why

- A pre-fix red for S3 (the suite without `tmux_server`): it would attach a client to the operator's default server, which the packet forbids. Instead, the positive check in `shipped_daemon_config` shows the daemon really uses its private server.
- No live daemon observation or deploy: the prime deploys after merge.
