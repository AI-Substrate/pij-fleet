# Plan 214 cost (AC7)

## What the change adds, per call site

| Site | Rate | Scaling | Launches |
|---|---|---|---|
| `ack_delivery` / urgent take: `INSERT ... held_deliveries SELECT ... WHERE id = ? AND acked_at - enqueued_at > 900` | once per delivery, inside the write the ack already makes | per message | 0 |
| PA digest: `unflagged_held_deliveries` (two partial indexes, `LIMIT 50`) and one flag write when it listed any | once per digest (fleet-idle edge or prime-active wake) | constant per digest, never per seat | 0 |
| s12 round: `message_reached_at` (three indexed probes) per open limited request still unchecked; one registry read only while still held | inside the existing 60 s watchdog round | per open limited request (normally a handful) | 0 |
| F68: one primary-key read of `role_requests` per send that carries `--in-reply-to` | per such send | per message | 0 |
| Copilot receiver heartbeat: `urgent` list from a daemon in-memory map | existing 20 s heartbeat | per native seat, unchanged rate | 0 |
| Copilot tool boundary: `urgentIds.size === 0` check; a daemon call only when an urgent message is pending | per tool call | per tool call, a set-size check | 0 |
| `POST /v1/inbox/urgent`: one indexed update (`jobs_by_dedupe`) per urgent message | per urgent message | per urgent message | 0 |
| `send.urgent` spine event | once per urgent send | per urgent message | 0 |

No new timer, poll, sweep or launch site; `crates/testkit/tests/spawn_sites.rs` is unchanged.
EXPLAIN guards: `the_held_read_never_scans_job_history` (held.rs) and `limit_admission_probes_are_indexed_searches` (role_requests.rs, now also the arrival probe).

## Live fleet, before (main, daemon pid 7956)

- `pij-rs --json ping | jq .data.spawns.total`: 566 at 2026-11-07T09:04:17Z and 566 at 09:05:17Z, so 0 daemon launches in 60 s at rest.
- `just spawn-rate --seconds 20`: 163.3 launches/s machine-wide (other seats' cargo and node; `daemon_children_per_s_lower_bound: {}`).
- Daemon: 28.7% CPU, 140 MB RSS.

## Live fleet, after

To take after this lands and the daemon is bounced: the same two `ping` readings 60 s apart, `just spawn-rate --seconds 20`, daemon CPU and RSS.
