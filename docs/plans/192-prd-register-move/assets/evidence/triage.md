# Triage of the earlier register's open rows (roadmap step s14, ruling C15)

Source: the earlier register at commit `e05661c8`, read with `git show` in a read-only checkout of the private repo where it stays. Nothing from it is copied here: no titles, no wording, no notes. Each row below has its old id, one verdict, the PRD 002 row it became (if kept), and a reason written for this record against the code on main at `b19c84b` (2026-11-04).

Verdicts: `kept` (written fresh into `docs/project/prds/002-pij-base.dd.json`), `dropped-met` (today's code already does it; the reason names the file or test), `dropped-unwanted` (the premise is gone, mostly because the TypeScript daemon generation is retired: `pij` is an rs-only shim, and `docs/how/pij-rs-api.md:3` says it never runs the TypeScript daemon).

Revised after review (reviewer verdict on b13f5b3): every kept row was re-checked against the code and its tests, and seven rows first kept were found already met and moved to `dropped-met`.

| old id | verdict | PRD 002 | reason |
|---|---|---|---|
| req-0001 | kept | req-04 | `pij-rs stream create` makes the worktree and branch (crates/daemon/src/orchestration/repo.rs:68-93); nothing under crates/daemon/src/orchestration/ removes a worktree or scaffolds a plan. |
| req-0002 | kept | req-01 | Spawns accept inbound by default (crates/cli/src/main.rs:2858-2864, tests claude_spawn_accepts_inbound_by_default and no_accept_inbound_withdraws_the_claude_default), and the reply address and control-shaped status are handled (crates/transport/src/uds.rs:249-256, 665-697). Only live proof of hold and deny is left: uds.rs:51-54 marks that decoder unproven on the daemon's connection, and its tests (1459-1593) use a stand-in socket. |
| req-0003 | dropped-met | | The reaper needs two witnesses (process gone and pane absent) and keeps a live process (crates/daemon/src/reaper.rs:2, tests reap_recheck_unknown_or_matching_live_process_cancels_candidate and reap_matching_live_process_is_retained_without_any_pane_probe_or_event in reaper_tests.rs:293, 314); the death sweep uses it (death_sweep.rs:2). A seat whose first registration failed re-registers with `pij-rs adopt <pane>`. |
| req-0004 | dropped-met | | The typing gate tracks and exempts pij's own injections, captures fresh before each send and expires a parked draft (crates/harnesses/src/interaction.rs:348-445, 486-609, 573-589; tests parked_draft_holds_at_most_the_idle_window_then_sends, self_echo_never_acquires_a_hold, self_echo_never_destroys_an_active_human_hold, fresh_boundary_replaces_a_stale_clear_composer_observation at 827, 877, 913, 979). |
| req-0005 | dropped-met | | Pane delivery stages the body in chunks as a bracketed paste and refuses lossy control bytes (crates/tmux/src/lib.rs:413-443, 972-977), the path every non-socket harness uses. |
| req-0006 | kept | req-02 | `pij-rs revive --help` has no preview option. Recycled-pid checks exist (docs/how/pij-rs-api.md:1018-1038), and re-binding a live seat is `pij-rs adopt <pane>`. |
| req-0008 | dropped-met | | Re-registration starts from the existing descriptor and sets Claude consent only when absent (crates/daemon/src/registration.rs:850-884; test ordinary_registration_preserves_unchanged_attested_native_owner_and_binding at 3467). |
| req-0009 | dropped-met | | Claude seats adopt themselves through the SessionStart hook the daemon installs (harness/scripts/claude-session-start-pij.sh, docs/how/claude-auto-seat.md). |
| req-0010 | kept | req-06 | Several verbs still answer E-RS-UNPORTED (.omp/extensions/pij/cli.ts:57). Merged with req-0016. |
| req-0011 | dropped-unwanted | | Asked for a generation-aware command with a legacy fallback; the shim is rs-only by design and refuses the retired generation (.omp/extensions/pij/core/generation-routing.ts). |
| req-0012 | kept | req-03 | crates/cli/src/main.rs:274-323 offers `--cwd` and no task field. |
| req-0013 | dropped-met | | The watchdog grades a seat by event, pane and work transitions, not screen text, and its own activity cannot mask a frozen peer (.omp/extensions/pij/core/watchdog.ts:194-240, watchdog.test.ts:230). |
| req-0014 | kept | req-08 | tsconfig.json:19 excludes `**/*.test.ts`. |
| req-0015 | dropped-met | | `pij inbox register` admits verified external hosts and `pij inbox --wait` gives them paneless pull (crates/daemon/src/registration.rs:139-235, crates/daemon/src/http/shim.rs:905-944). |
| req-0016 | kept | req-06 | Peer transcript tail is refused (.omp/extensions/pij/core/generation-routing.ts:303) while `pij-rs tail` follows daemon events (crates/cli/src/main.rs:1702-1727). Merged with req-0010. |
| req-0017 | kept | req-07 | `pij generation` is only a routing check (.omp/extensions/pij/cli.ts:71-75); the daemon's routes (crates/daemon/src/http/mod.rs:596-660) include no daemon listing. |
| req-0018 | dropped-met | | A periodic death sweep tombstones seats observed dead by both witnesses (crates/daemon/src/death_sweep.rs:81-95, started from crates/daemon/src/lib.rs; test death_sweep_matches_manual_candidates_and_commits_observation_once), and a send to a tombstoned seat is refused (crates/daemon/src/delivery/mod.rs:4306-4326). |
| req-0020 | kept | req-13 | Paneless registration refuses unproven callers (crates/daemon/src/registration.rs:183-251); there is no grade. Kept as `could`, planned, by ruling C17. |
| req-0022 | dropped-met | | Same as req-0005: chunked bracketed paste (crates/tmux/src/lib.rs:413-443). |
| req-0026 | kept | req-05 | crates/harnesses/src/spawn.rs:142-146 passes PIJ_SESSION_ID and PIJ_SPAWN_ID to the child, not PIJ_RS_ADDR or PIJ_RS_STATE_DIR. |
| req-0027 | dropped-met | | `pij-rs send` and the shim both take `--body-file` (crates/cli/src/main.rs:354-358, 2173-2199). |
| req-0028 | dropped-met | | No test binds a fixed port; fixtures across crates/, harness/ and the extensions bind port 0. |
| req-0029 | dropped-met | | Recycled pids are told apart by pid and start time (crates/core/tests/liveness_table.rs), acks to the wrong recipient are refused (crates/daemon/src/http/tests.rs:813), the inbox-ack audit event is asserted in crates/cli/tests/spawn.rs, and there is no legacy fallback. |
| req-0030 | dropped-met | | Same gate as req-0004; the delivery service consults it at line 223 of crates/daemon/src/delivery/mod.rs, with tests at crates/harnesses/src/interaction.rs:1204 and 1311. |
| req-0032 | dropped-met | | `pij-rs send` mints the message id when it is omitted and refuses a missing body by name. |
| req-0034 | dropped-unwanted | | About a legacy spawner and its children; the legacy generation is retired. |
| req-0038 | dropped-unwanted | | Work inside another project that reads pij's API; pij already serves /v1/seats and /v1/events. |
| req-0039 | dropped-unwanted | | A migration from the legacy generation, which no longer runs. |
| req-0040 | dropped-met | | Seats carry a role (`pij-rs spawn --role`, `pij-rs role`) and registration writes semantic state (crates/daemon/src/registration.rs). |
| req-0041 | dropped-met | | `pij-rs send` derives the sender when `--from` is omitted and mints the id (`pij-rs send --help`). |
| req-0042 | dropped-met | | Socket delivery skips the composer and pointer path (crates/daemon/src/pointer/worker.rs:2315, socket_body_is_delivered_and_acked_without_pointer_or_retry). |
| req-0043 | dropped-unwanted | | Asked status bars to tag the legacy generation; with rs-only there is nothing to tag. |
| req-0044 | dropped-met | | Main CI is green, and the worktree test named in the row no longer exists. |
| req-0045 | kept | req-09 | /v1/send takes the body's `from` and refuses only pij-bg (crates/daemon/src/http/mod.rs:1689-1700); delivery routes that sender with no registry lookup (crates/daemon/src/delivery/mod.rs:991-1025). |
| req-0047 | dropped-met | | A one-off boot fault in an old build; seat queries now read whole rows (crates/store/src/registry.rs). |
| req-0048 | kept | req-10 | The extension accepts only envelope v2 (.omp/extensions/pij/core/daemon-wire.ts:263-265), and `just bounce-rs` (justfile:569-583) sends no fleet notice. |
| req-0050 | dropped-met | | `harness checks` runs a `rust` stage (`just rust-check`, .harness/extensions/checks/extension.ts). |
| req-0052 | kept | req-11 | crates/store/src/queue.rs:792-799 has an internal peek; no route in crates/daemon/src/http/mod.rs:596-660 exposes a seat's queue. |
| req-0053 | dropped-met | | The Copilot footer matcher accepts any line starting `/ commands` or containing `context` (crates/harnesses/src/composer.rs:188-194), so a wrapped footer's first line still matches; held receipts carry the persisted retry time (crates/daemon/src/delivery/mod.rs:3869). |
| req-0054 | kept | req-12 | `delivery.outcome` events are keyed to the recipient (crates/daemon/src/delivery/mod.rs:32-34, 854); nothing reaches the sender. The agent-output half is met (crates/harnesses/src/interaction.rs:1039). |

## Evidence for each PRD 002 row (what is still unmet today)

- req-01: crates/transport/src/uds.rs:51-54 (hold/deny decoder unproven on the daemon's connection; tests at 1459-1593 use a stand-in socket).
- req-02: `pij-rs revive --help` (no preview option).
- req-03: crates/cli/src/main.rs:274-323 (spawn arguments: no task field).
- req-04: crates/daemon/src/orchestration/repo.rs:68-93 (create only; no worktree removal or plan scaffold anywhere in crates/daemon/src/orchestration/).
- req-05: crates/harnesses/src/spawn.rs:142-146 (child env has no PIJ_RS_ADDR or PIJ_RS_STATE_DIR).
- req-06: .omp/extensions/pij/cli.ts:57 and .omp/extensions/pij/core/generation-routing.ts:303 (verbs answered E-RS-UNPORTED).
- req-07: crates/daemon/src/http/mod.rs:596-660 (no daemon listing route); .omp/extensions/pij/cli.ts:71-75.
- req-08: tsconfig.json:19 (tests excluded from typecheck).
- req-09: crates/daemon/src/http/mod.rs:1689-1700 and crates/daemon/src/delivery/mod.rs:991-1025 (asserted sender, no registry check).
- req-10: .omp/extensions/pij/core/daemon-wire.ts:263-265 (v2 only); justfile:569-583 (no fleet notice).
- req-11: crates/store/src/queue.rs:792-799 (internal peek only); crates/daemon/src/http/mod.rs:596-660 (no queue route).
- req-12: crates/daemon/src/delivery/mod.rs:854 (outcome published to the recipient's key).
- req-13: crates/daemon/src/registration.rs:183-251 (unproven callers refused; no grade). Ruling C17.
