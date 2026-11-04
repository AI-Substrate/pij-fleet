# Triage of the earlier register's open rows (roadmap step s14, ruling C15)

Source: the earlier register at commit `e05661c8`, read with `git show` in a read-only checkout of the private repo where it stays. Nothing from it is copied here: no titles, no wording, no notes. Each row below has its old id, one verdict, the PRD 002 row it became (if kept), and a reason written for this record against the code on main at `79b535b` (2026-11-04).

Verdicts: `kept` (written fresh into `docs/project/prds/002-pij-base.dd.json`), `dropped-met` (today's code already does it), `dropped-unwanted` (the premise is gone, mostly because the TypeScript daemon generation is retired: `pij` is an rs-only shim, and `docs/how/pij-rs-api.md` says it never runs the TypeScript daemon).

| old id | verdict | PRD 002 | reason |
|---|---|---|---|
| req-0001 | kept | req-08 | `pij-rs stream` opens and closes streams, but nothing scaffolds plan documents or reclaims per-seat build output. |
| req-0002 | kept | req-05 | The socket transport exists in crates/transport/src/uds.rs but is off unless `--accept-inbound` is passed; the default is an operator call. |
| req-0003 | kept | req-01 | No code holds back an exit notice for a seat still between spawn and registration. |
| req-0004 | kept | req-03 | The typing gate is consulted in crates/daemon/src/delivery/mod.rs; idle expiry and the self-echo exemption are missing. Merged with req-0030. |
| req-0005 | kept | req-04 | Pane submit exists in crates/tmux/src/lib.rs; there is no live proof per harness. Merged with req-0022. |
| req-0006 | kept | req-06 | `pij-rs revive` has `--assume-dead` but no preview and no re-attach of a live unbound seat. |
| req-0008 | kept | req-02 | No test in crates/daemon/src/registration.rs or crates/store/tests shows re-registration keeping consent and transport fields. |
| req-0009 | dropped-met | | Claude seats adopt themselves through the SessionStart hook the daemon installs (docs/how/claude-auto-seat.md). |
| req-0010 | kept | req-11 | Several verbs still return E-RS-UNPORTED in `pij --help`. Merged with req-0016. |
| req-0011 | dropped-unwanted | | Asked for a generation-aware command with a legacy fallback; the shim is rs-only by design and has no legacy path. |
| req-0012 | kept | req-07 | `pij-rs spawn --cwd` sets the directory; there is no way to pass a first task. |
| req-0013 | kept | req-13 | The watchdog in .omp/extensions/pij/core/watchdog.ts does not require a liveness signal before a notice. |
| req-0014 | kept | req-14 | tsconfig.json excludes `**/*.test.ts`, so tests are not typechecked. |
| req-0015 | dropped-met | | `pij inbox register` admits verified external hosts and `pij inbox --wait` gives them paneless pull. |
| req-0016 | kept | req-11 | `pij tail` is refused because peer transcript tail differs from `pij-rs tail`; the naming is not settled. Merged with req-0010. |
| req-0017 | kept | req-12 | Only `pij generation` exists; nothing names another daemon on a miss or lists running daemons. |
| req-0018 | kept | req-09 | `pij-rs reap` reconciles stale bindings when run by hand; there is no automatic tombstone. |
| req-0020 | kept | req-20 | There is no proven/unproven grade; registration admits verified hosts only. Kept as `could` on pm-001's call; whether it is wanted at all is open for the prime (PRD 002 q-02). |
| req-0022 | kept | req-04 | Pane typing exists; a live first-attempt proof for long bodies per harness does not. Merged with req-0005. |
| req-0026 | kept | req-10 | Spawned seats get PIJ_SESSION_ID and PIJ_SPAWN_ID but not the daemon's address or state directory. |
| req-0027 | dropped-met | | `pij-rs send` and the shim both take `--body-file` (see `pij-rs send --help`). |
| req-0028 | dropped-met | | No test binds a fixed port; fixtures across crates/, harness/ and the extensions bind port 0. |
| req-0029 | dropped-met | | Recycled pids are told apart by pid and start time (crates/core/tests/liveness_table.rs), acks to the wrong recipient are refused (crates/daemon/src/http/tests.rs), the inbox-ack audit event is asserted in crates/cli/tests/spawn.rs, and there is no legacy fallback. |
| req-0030 | kept | req-03 | Same need as req-0004, for the rs path. Merged. |
| req-0032 | dropped-met | | `pij-rs send` mints the message id when it is omitted and refuses a missing body by name. |
| req-0034 | dropped-unwanted | | About a legacy spawner and its children; the legacy generation is retired. |
| req-0038 | dropped-unwanted | | Work inside another project that reads pij's API; pij already serves /v1/seats and /v1/events. |
| req-0039 | dropped-unwanted | | A migration from the legacy generation, which no longer runs. |
| req-0040 | dropped-met | | Seats carry a role (`pij-rs spawn --role`, `pij-rs role`) and registration writes semantic state (crates/daemon/src/registration.rs). |
| req-0041 | dropped-met | | `pij-rs send` derives the sender when `--from` is omitted and mints the id. |
| req-0042 | dropped-met | | Socket delivery skips the composer and pointer path (crates/daemon/src/pointer/worker.rs, socket delivery test). |
| req-0043 | dropped-unwanted | | Asked status bars to tag the legacy generation; with rs-only there is nothing to tag. |
| req-0044 | dropped-met | | Main CI is green at 79b535b, and the worktree test named in the row no longer exists. |
| req-0045 | kept | req-15 | Sends from a registered pane are derived, but no check shows an unregistered caller is refused or marked. |
| req-0047 | dropped-met | | A one-off boot fault in an old build; seat queries now read whole rows (crates/store/src/registry.rs). |
| req-0048 | kept | req-16 | The extension rejects any wire version mismatch (.omp/extensions/pij/core/daemon-wire.ts); there is no N-1 tolerance. |
| req-0050 | dropped-met | | `harness checks` runs a `rust` stage (`just rust-check`, .harness/extensions/checks/extension.ts). |
| req-0052 | kept | req-17 | crates/store/src/queue.rs has an internal peek; no verb exposes it. |
| req-0053 | kept | req-19 | Footer recognition in crates/harnesses/src/composer.rs has no wrapped-footer case. |
| req-0054 | kept | req-18 | Senders are not told later when a queued message lands. |
