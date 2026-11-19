# Plan 231 proof

Branch `plan-231-spawn-cache-line` from main `44ec05e9`. The binary was built with `just shared-build -- build -p pij-cli --bin pij-rs` and copied to `/tmp/p231/pij-rs` straight after its build (D-229-2); `spawn --help` names `--no-cache-proxy`. It ran against the live daemon and the live sidecar on 127.0.0.1:7471, 2026-11-19.

| AC | Command | Observed |
|---|---|---|
| ac-0001 | `pij-rs spawn --harness copilot --model claude-haiku-4.5 --no-wait --cwd /tmp/p231 --name smoke-231` | `pij spawn: ok — {…}` then `cache: proxied (1-hour cache through http://127.0.0.1:7471 as claude-haiku-4.5)` |
| ac-0002 | `pij-rs revive pij-no-such-seat-231` | `pij revive: FAILED — seat … does not exist …`; no cache line |
| ac-0002 | `pij-rs --json spawn --harness copilot --model claude-haiku-4.5 --no-wait …` | one line: the envelope, `data.cache_proxy` = `{state: proxied, reason: "1-hour cache through http://127.0.0.1:7471 as claude-haiku-4.5"}` |
| ac-0001, ac-0003 | `cargo test -p pij-cli --bin pij-rs a_copilot_receipt_line` (shared-build) | 1 passed: proxied; plain with fix; plain without fix; before the gate; non-Copilot gives none; a reason and fix carrying `\n` and a forged `cache: proxied` stay on one line, fenced |
| all | `cargo clippy -p pij-cli --all-targets -- -D warnings` (shared-build) | clean |
| ac-0004 | docs/how/pij-rs-api.md § Copilot cache proxy gate | reviewer judgement |

Both smoke seats (pij-territorial-stork, pij-additional-rook) had their panes killed straight after; the death sweep retires them.

## Round 2 (S246-001, S246-002)

| Finding | Change | Observed |
|---|---|---|
| S246-001 | `execution_guardrails` set through `ddocs set` to a repo-relative worktree name; `ddocs build` and `ddocs validate` | no absolute home-directory path left under `docs/plans/231-spawn-cache-line/` |
| S246-002 | the human line, and the `pij directory` holder rows, use fixed words only (`public_cache_reason`) | `cargo test -p pij-cli` (shared-build): `the_receipt_line_never_shows_the_account_or_a_local_path` and `a_copilot_receipt_line_says_the_cache_route` pass, along with the rest of the crate; clippy `-D warnings` clean |

The proxied line is unchanged from round 1's live proof: the daemon's own reason is kept as is.
