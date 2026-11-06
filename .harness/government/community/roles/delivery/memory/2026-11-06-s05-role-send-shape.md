# s05 send to a role: what shipped and where the seams are (plan 199, PR #129)

- **date:** 2026-11-06
- **tags:** s05, s06, s12, role-send, role-queue, migration-0034, reply-modes, drain, pa-digest
- **hook:** Read before s06 or s12: how a role request is stored, routed, queued, woken and drained, and the two traps (ddocs set reformats; a #122 timing flake).

## Shape (PR #129, head da23eb7)

- **Store** `crates/store/src/role_requests.rs`, migration 0034 `role_requests`: `id` = the send's msg_id (the request id, C3), project, role, instance ('' = any holder), requester, body, in_reply_to, fyi, reply `wake|fyi|none`, limit_ms, state `queued|delivered`, holder, created_at, delivered_at. API: `record_role_request`, `role_request(id)`, `queued_role_requests(project, role, instance)` (instance-less requests match any instance, oldest first), `all_queued_role_requests`, `mark_role_request_delivered` (true at most once).
- **Route** `crates/daemon/src/delivery/role_send.rs::route`: project from `--project` or the caller's cwd (`http::directory::project_and_repo`), role doc via `community::root::find` (no git launch), live holders = open assignments + registry not tombstoned. 1 holder → `Routed::Holder` (body gets a `[pij role:… request <id> from <seat> · reply … · limit …]` header line); 0 → queued + `wake` (project's `pa` holder, else prime designation, else prime-role seat, through `pa_watchdog::wake_pa`, msg_id `role-queue-<id>`); ≥2 → `E-RS-ROLE-HOLDERS`. C1 mismatch → `E-RS-ROLE-MISMATCH` (prime exempt; no recorded role passes).
- **HTTP** `crates/daemon/src/http/role_send.rs::route` is called by both `/v1/send` (`SendBodyRequest` has top-level `reply`, `limit`, `project`) and the shim (`SendCall` has the same three). Receipt = ordinary receipt + `request {...}` (+ `woke` when queued).
- **Drain (C6)** `RoleQueueDrain` replaces `NoHook` in `lib.rs`; spawns `drain()`: each queued request delivered from its requester (FYIs via `hold_fyi`), then `mark_role_request_delivered`.
- **Digest**: `pij_core::pa_digest::FleetView.queued` filled by `pa_watchdog::queued_requests` (one read per nudge); renders only when non-empty.

## For s06 (notify) and s12 (limits)

- s06: find the request by `--answers <msg_id>` = `role_request(id)`; requester and `reply` are on it. Nothing marks a request "answered" yet; s06 owns that state.
- s12: `limit_ms` is stored (explicit `--limit`, else the role's `default_limit` at send time). The clock should start at `delivered_at` (my 11-04 note, req-22 §3), which the drain and the holder path both set.

## Traps

- `ddocs set` on `docs/project/*.dd.json` rewrites the whole tab-indented file; edit the field text in place, then `ddocs build` and `ddocs validate`.
- `directory_http::review::review_reassign_within_one_second_keeps_both_holdings` (#122) fails locally under load when assign/release/reassign cross a second boundary; it passed in CI.
- Builds: `PIJ_SHARED_TARGET=… just shared-build -- …`; other lanes hold the lock for minutes, so batch with `--then`.
