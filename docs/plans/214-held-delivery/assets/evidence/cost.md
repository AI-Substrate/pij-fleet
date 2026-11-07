# Plan 214 cost (AC7) and claim-wait numbers (AC1)

## Where the hold is (AC1)

For delivery jobs, `enqueued_at` is queued, `claimed_at` the recipient's claim and `acked_at` reached-the-session. Read-only queries on the live store (`sqlite3 -readonly ~/.pij-rs/pij.sqlite`, `kind LIKE 'delivery:%' AND state='done'`, the last 24 hours, seats with 10 or more deliveries) show the whole hold is between queue and claim; once claimed, a message reaches the session within seconds.

2026-11-06 (first look, Copilot seats):

| Seat | Deliveries | Claim wait > 15 min | Max claim wait | Max claim to ack |
|---|---|---|---|---|
| pij-local-warbler | 54 | 39 | 449 min | 0 min |
| pij-back-tapir | 49 | 32 | 242 min | 0 min |
| pij-swift-kestrel | 39 | 30 | 241 min | 0 min |
| pij-wary-lynx | 48 | 20 | 179 min | 0 min |
| pij-exuberant-wildcat | 36 | 19 | 117 min | 0 min |

2026-11-07T11:11Z (before this plan deploys):

| Seat | Deliveries | Claim wait > 15 min | Mean claim wait | Max claim wait | Max claim to ack |
|---|---|---|---|---|---|
| pij-back-tapir | 13 | 9 | 412 min | 1209 min | 5 s |
| pij-local-warbler | 11 | 5 | 33 min | 108 min | 2 s |
| pij-steady-otter (pm-001) | 160 | 0 | 0.2 min | 2 min | 21 s |
| every other seat with 10+ | | 0 | under 2 min | 12 min | 3 s |

Cause: the Copilot extension waited for the turn its last pij message started to end before claiming the next one (the post-ack barrier). Ruling C31 lifts it per seat (`pij steer`, on by default). After numbers are taken from the same query once the daemon and extension are deployed.

## What the change adds, per call site

| Site | Rate | Scaling | Launches |
|---|---|---|---|
| `ack_delivery`: `INSERT ... held_deliveries SELECT ... WHERE id = ? AND acked_at - enqueued_at > 900` | once per delivery, inside the write the ack already makes | per message | 0 |
| PA digest: `unflagged_held_deliveries` (two partial indexes, `LIMIT 50`) and one flag write when it listed any | once per digest (fleet-idle edge or prime-active wake) | constant per digest, never per seat | 0 |
| Native claim: `mid_turn_steering` (one primary-key read of `seat_steering`) | per claim (a long poll), not per tick | per message | 0 |
| Native Copilot receiver during a pij-started turn: one more claim per message during the turn instead of one after it | per message | per message | 0 |
| Send: receipt note, one registry read (plus the steering read for native Copilot) only for a queued send | per queued send | per message | 0 |
| s12 round: `message_reached_at` (three indexed probes) per open limited request still unchecked; one registry read only while still held | inside the existing 60 s watchdog round | per open limited request | 0 |
| F68: one primary-key read of `role_requests` per send that carries `--in-reply-to` | per such send | per message | 0 |
| `pij steer`: one write and one audit event per real change | per command | per command | 0 |

No new timer, poll, sweep or launch site; `crates/testkit/tests/spawn_sites.rs` is unchanged.
EXPLAIN guards: `the_held_read_never_scans_job_history` (held.rs) and `limit_admission_probes_are_indexed_searches` (role_requests.rs, now also the arrival probe).

## Live fleet, before (main, daemon pid 7956)

- `pij-rs --json ping | jq .data.spawns.total`: 566 at 2026-11-07T09:04:17Z and 566 at 09:05:17Z, so 0 daemon launches in 60 s at rest.
- `just spawn-rate --seconds 20`: 163.3 launches/s machine-wide (other seats' cargo and node; `daemon_children_per_s_lower_bound: {}`).
- Daemon: 28.7% CPU, 140 MB RSS.

## Live fleet, after

To take after this lands and the daemon is bounced: the same two `ping` readings 60 s apart, `just spawn-rate --seconds 20`, daemon CPU and RSS, and the claim-wait query above.
