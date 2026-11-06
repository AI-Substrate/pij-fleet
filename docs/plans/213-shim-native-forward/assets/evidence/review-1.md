# Plan 213: review round 1 (#147 at 29488ca)

Reviewer (pij-pleased-marmot) blocked on F001 and F002. Security (pij-wary-lynx) and performance passed with notes. All folded into one push.

## F001 (P2): value-taking send flags misrouted the selector

`--body`, `--from` and `--msg-id` were missing from the shim's value-flag set. So `send --body hello --to role:probe` and `send --from x role:probe hello` stayed routed, and `send --msg-id role:fake pij-peer hello` was forwarded as a role send.

Fix:
- `pij-native-verbs.json` now carries `send_value_flags` and `global_value_flags`, and a clap test (`the_shims_value_flag_tables_are_claps`) holds them equal to pij-rs's value-taking options. A new value flag missing from the table is caught (negative control).
- `routed_send_value_flags` (`--file`, `--caption`) are shim-only, and the same test proves pij-rs has neither.
- `scanSend` reads every flag with its value, in both the separate and the `=` spelling, and honours `--`.

Tests (`crates/cli/tests/shim_forward.rs`):
- Every send value flag, carrying a `role:`-looking value ahead of a role recipient, is forwarded unchanged, in both spellings.
- The same flag ahead of a seat recipient stays routed.
- The reviewer's three orders and its `--msg-id role:fake` case are included.

## F002 (P2): an inherited `PIJ_SESSION_ID` skipped paneless admission

A paneless child that inherited its parent's seat id was forwarded, so the native resolver could send as the parent.

Fix: a send is forwarded only from a tmux pane, the identity the daemon observes. Every paneless send keeps the routed path, which registers the observed host and overrides the inherited id. That override is covered by `generation-router.test.ts`, "%s resolves ambient identity before forwarding its untouched argv".

Tests: a paneless child with `PIJ_SESSION_ID=pij-parent-fixture` beside its own `COPILOT_AGENT_SESSION_ID` stays routed, both for `role:` and for `--to`.

## N001 (P3): a leading `--json` missed the fast path

Fix: `findVerb` skips pij-rs's global options (`--json`, `--state-dir`, `--addr`, and their `=` forms) and passes argv unchanged.

Tests: `--json <verb>` and `--state-dir … --addr=… <verb>` for every community verb, `--json send role:…`, and `--addr … --json send --help`.

## Security notes (lynx)

- **PATH:** `pij-rs` is resolved once per call, from absolute PATH entries only (relative and empty entries are skipped, as `ddocs-check.ts` does), and spawned by that absolute path. Test: `a_relative_path_entry_never_supplies_pij_rs`.
- **whoami:** native `resolve_acting_seat` now sends the full `caller_context()` with seat and pane, so the daemon's `codex_caller::refusal_for` check runs as it does on the routed path. This covers send, compact-self, inbox and sidecar telegram.

## Selector check before pushing

The reviewer's oracle cases plus N001, help, `--file` and leading global flags (18 cases in all) were run against the new `forwardsToPijRs`. All 18 were right. The full proof is CI on the head.
