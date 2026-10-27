# Review fixes for PR #73 (plan 180)

Fixer seat `pij-storm-fixes`, for prime `pij-far-jackal` (Jordan: "do it"). Review: `pij-review-pr73`, verdict *merge after fixes*, 0 blockers, 4 should-fix, 7 nits. Fixed here: S1 to S4 and nit 4. The other nits go to a follow-up issue the prime files. Rulings: `../rulings.md` R-PRIME-1 to R-PRIME-4 (R3 and R5 amended by excision).

Every tmux experiment and test ran on a private `tmux -L` server, killed afterwards. The operator's default server was only ever read (`list-clients`, `list-sessions`) for the rule-3 proof.

## S1: the control client gets its own session, `_pij`

- `crates/tmux/src/control.rs`: `CONTROL_SESSION`, and `start()` runs one launch, `tmux -u -N -C new-session -A -s _pij -f no-output,ignore-size,read-only 'exec sleep 2147483647'`. `-N` means no server is ever started; `-A` attaches if `_pij` exists. Before the handshake the client subscribes to `#{server_sessions}` and sets up `destroy-unattached` for `_pij` (see the recheck section below); `Framer` turns `%subscription-changed pij-sessions … : 1` into `Frame::Alone`. The client then leaves, `_pij` (holding only its own pane) is destroyed, and an emptied server exits as it did before. The restart after `Alone` waits out `RESTART_BACKOFF`. `first_session()` (a `list-sessions` launch) is gone, so warm-up is now 1 launch, not 2.
- `crates/tmux/src/lib.rs` `list_panes`: never returns the control pane itself (narrowed at recheck from the whole session). Every consumer (pane observer reconcile/tap sweep, reaper, revive, caller-pane resolution, pointer worker, interaction gate) gets panes only from `list_panes`, so none of them can see it.
- The prime's stop condition triggered: untargeted commands from outside tmux still resolve to `_pij` after each control attach, until a keypress elsewhere (`exp3.py`) and indefinitely when no human is attached. The prime ruled to ship with this residual, documented and pinned (R-PRIME-1), and at recheck made what lands there persist and stay visible.
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

## Recheck at 4f062c5: R1 and nit 3

The recheck (`pij-review-pr73`) found that R-PRIME-1's "no worse than the old design" was false: a window that landed in `_pij` was hidden from pij and then destroyed with the control client, and with no human attached the resolution residual never ends. The prime ruled four changes:

- **R1, windows that land in `_pij` persist** (`control.rs` `start()` setup). `_pij` gets session hooks `after-new-window` and `after-split-window` → `set-option destroy-unattached off`. A later client re-arms `destroy-unattached on` only through `if-shell -F` when `_pij` holds a lone pane, so it cannot undo the protection. Tests: `a_foreign_window_in_pij_stays_listed_and_outlives_the_control_client` (including a second client re-attaching) and `a_foreign_pane_split_into_pij_stays_listed_and_outlives_the_control_client`. `pij_with_only_its_own_window_goes_with_the_client` guards the old behaviour. `the_control_session_is_attached_read_only_and_never_listed` now also asserts the hooks and option are `_pij`'s, never global.
- **The filter covers the control pane only** (`lib.rs` `list_args`). `list-panes -f` with `control::NOT_CONTROL_PANE` matches the pane's start command (`*sleep 2147483647*`; tmux 3.6 reports it quoted) instead of the whole session, so a foreign window in `_pij` is listed.
- **Found along the way, and fixed:** once a foreign window keeps `_pij` alive, the old `#{server_sessions} == 1` subscription would make the client leave and re-attach every 5 s whenever `_pij` was the last session. "Alone" now also requires `_pij` to hold only the control pane (`ALONE`). Test: `a_foreign_window_keeps_the_client_attached_when_pij_is_the_last_session`.
- **Nit 3, spawning into `_pij` is refused** (`daemon/src/http/mod.rs` `control_session_refused`). It is refused with `details.code = E-RS-CONTROL-SESSION`, whether `--session _pij` was named or `_pij` came from the caller's pane, and before any launch or roster write. Test: `spawn_into_the_control_session_is_refused_by_name` pins the text.
- **Wording:** R-PRIME-1, `macos-defender.md` and the PR body were rewritten, not appended to: a foreign window in `_pij` persists and stays visible, and the residual lasts until a keypress elsewhere, or indefinitely with no human attached.

| test | red against | observed red |
|---|---|---|
| `a_foreign_window_in_pij_stays_listed_and_outlives_the_control_client` | 4f062c5 | `a foreign window in _pij is hidden from pij`; with the filter only, `destroyed with the client` |
| `a_foreign_pane_split_into_pij_stays_listed_and_outlives_the_control_client` | 4f062c5 | `%4 hidden`; with the filter only, `destroyed with the client` |
| `a_foreign_window_keeps_the_client_attached_when_pij_is_the_last_session` | the session-count subscription | `left: [] right: [6299]` (the client left) |
| `spawn_into_the_control_session_is_refused_by_name` | 4f062c5 | `left: 200 right: 400` |

| mutant | result |
|---|---|
| re-arm `destroy-unattached on` unconditionally | killed: `destroyed by the second client` |
| drop the `after-split-window` hook | killed: split test `destroyed with the client` |

Under load average 56, `adapter.rs` timing tests failed once (`separate_adapters_cannot_both_reserve_one_pane`, `committed_marker_transition_failure_never_sends_enter`) and passed 29/29 alone. The recheck reports the same flakes.
