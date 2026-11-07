# Plan 221 phase 1 evidence

## AC1: route inventory

- `harness/scripts/route-inventory.json` lists the 64 (method, path) pairs main serves at 2cc2ac3: 43 seat-authority, 20 machine-public and 1 job-token (`POST /v1/bg/{job}/emit`). It starts from the F88 note (63 pairs at a0ec952) and adds `POST /v1/requests` (#149, its actor resolved by R; withdraw and close need the requester, the PA or the prime). #153 and #156 changed handlers but added no route. #152 (steering) is not merged; when it adds a route, the sensor will make that PR classify it.
- Guards use the note's codes (R, B, N, S, C, the provenance exception as P), plus A for "asserted": `/v1/send` takes `from` as given.
- The F88 note's "4 other" pairs: hold, release and sidecar are machine-public with their reasons, and the emit hook is `job-token`. That is a third authority beside the AC's two, because the hook takes neither the daemon key nor a seat's authority. The checker requires `job-token` exactly where the router mounts a route outside the bearer layer (`RouteAuth::JobToken`).
- **The sensor:** `crates/daemon/src/http/route_inventory_tests.rs`, run by `just route-inventory-check`.
  - It probes a router built by `router_with_config` (fakes, a scratch store, the local key) for every `Endpoint::ALL` path under GET, POST, PUT, PATCH and DELETE, with no body; a pair counts as served unless axum answers 405. That catches the five second methods that `Endpoint::method` doesn't record (`GET /v1/bg`, `POST /v1/seats`, `GET /v1/whoami`, `GET /v1/anomalies`, `GET /v1/decisions`).
  - It never reads the JSON to decide what is served.
  - Wiring: a heavy `route-inventory` stage in `.harness/extensions/checks/` (skipped by `--quick`), a step in `just self-check`, `.harness/extensions/checks/instructions.md`, `.harness/engineering-harness.md` and AGENTS.md's inventory line.
- **Tests, one per failure:**
  - a served route missing from the inventory, including a second method on a listed path;
  - a stale entry and a repeated entry;
  - an unclassified or misclassified route: no class, empty `grants_by`, a repeated or unknown guard, an unknown authority, machine-public with a blank reason;
  - `job-token` on the wrong route, or missing from the right one;
  - a malformed file.
- **Mutation check on the real probe:** removing `/v1/requests` from the JSON, adding a `GET /v1/ghost` entry and deleting `/v1/send`'s class gave exactly three findings, one per change, and the test failed.
- Local, niced, through `just shared-build`: `test -p pij-daemon --lib route_inventory` passed 7 of 7.

## AC2: the workshop

[`workshops/001-capability-bootstrap.md`](../../workshops/001-capability-bootstrap.md), commit bf89eed.
- It recommends option C: a single-use spawn nonce, or Unix-socket peer credentials, as bootstrap evidence; a hashed per-incarnation token as the per-request control; peer-credential ancestry for the shim and hooks.
- Phase 2's order: send-as, inbox-claim, controls, role-and-holder, outputs, registration and adoption, activity, attribution.
- Pending: security and reviewer verdicts at that commit.

## AC3: cost

Phase 1 adds no daemon runtime cost. The only daemon source change is a `#[cfg(test)]` module declaration: no loop, poll, timer, hook or launch site, so `spawn_sites.rs` and the live spawn readings are unchanged by construction. The sensor runs once per `harness checks` (or `self-check`) invocation: one router probe of 59 paths × 5 methods against fakes, plus one JSON read. The workshop states the chosen design's per-request cost: one SHA-256 and a constant-time compare for token requests; for peer-credential calls, one `getsockopt`, plus an ancestry walk on a cache miss at agent-action rate. Nothing launches per tick per seat.

## AC4: docs and states

- `docs/how/pij-rs-api.md` has a new "Seat authority" section pointing at the inventory, the sensor and the workshop.
- `docs/difficulties.md` records F88 as D-221-1, open until phase 2's gates ship.
- The plan's implementation summary states phases 2 and later as follow-up waves.
- No new verbs or refusals.
