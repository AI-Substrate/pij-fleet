# Plan 221 phase 1 evidence

## AC1: route inventory

- `harness/scripts/route-inventory.json` lists the 76 (method, path) pairs main serves: 46 seat-authority, 29 machine-public and 1 job-token (`POST /v1/bg/{job}/emit`). Of those, 12 are implicit HEAD pairs: axum serves HEAD through each GET handler, effects included, and each takes its GET's classification. Round 2, spoonbill F002, found them; the first probe skipped HEAD. It starts from the F88 note (63 pairs at a0ec952) and adds `POST /v1/requests` (#149, its actor resolved by R; withdraw and close need the requester, the PA or the prime). #153 and #156 changed handlers but added no route. #152 (steering) is not merged; when it adds a route, the sensor will make that PR classify it.
- Guards use the note's codes (R, B, N, S, C, the provenance exception as P), plus A for "asserted": `/v1/send` takes `from` as given.
- The F88 note's "4 other" pairs: hold, release and sidecar are machine-public with their reasons, and the emit hook is `job-token`. That is a third authority beside the AC's two, because the hook takes neither the daemon key nor a seat's authority. The checker requires `job-token` exactly where the router mounts a route outside the bearer layer (`RouteAuth::JobToken`).
- **The sensor:** `crates/daemon/src/http/route_inventory_tests.rs`, run by `just route-inventory-check`.
  - It probes a router built by `router_with_config` (fakes, a scratch store, the local key) for every `Endpoint::ALL` path under every standard method: GET, HEAD, POST, PUT, PATCH, DELETE and OPTIONS. Each call has no body and a 5 s timeout, and the whole enumeration a 60 s timeout, so a hang fails. A pair counts as served unless axum answers 405. OPTIONS is served nowhere. That catches the five second methods that `Endpoint::method` doesn't record (`GET /v1/bg`, `POST /v1/seats`, `GET /v1/whoami`, `GET /v1/anomalies`, `GET /v1/decisions`).
  - It never reads the JSON to decide what is served.
  - Wiring: a heavy `route-inventory` stage in `.harness/extensions/checks/` (skipped by `--quick`), a step in `just self-check`, `.harness/extensions/checks/instructions.md`, `.harness/engineering-harness.md` and AGENTS.md's inventory line.
- **Tests, one per failure:**
  - a served route missing from the inventory, including a second method on a listed path and an omitted implicit HEAD;
  - a stale entry and a repeated entry;
  - an unclassified or misclassified route: no class, empty `grants_by`, a repeated or unknown guard, an unknown authority, machine-public with a blank reason;
  - `job-token` on the wrong route, or missing from the right one;
  - a malformed file.
- **Mutation check on the real probe:** removing `/v1/requests` from the JSON, adding a `GET /v1/ghost` entry and deleting `/v1/send`'s class gave exactly three findings, one per change, and the test failed.
- Local: `just route-inventory-check` passed 6 of 6, with the probe test finding exactly the 76 pairs.

## AC2: the workshop

[`workshops/001-capability-bootstrap.md`](../../workshops/001-capability-bootstrap.md).
- First draft at bf89eed. Revision 1 in this head answers the three blocking verdicts at bf89eed:
  - performance, ocelot, P163-F001 (governance b78df40): in-process ancestry with a strict no-launch refusal, a bounded walk, a cache with an explicit lifetime, and the cost budget;
  - security, squirrel (governance f1d74f3): session ownership for shared Copilot hosts, and contracts a to f;
  - reviewer, bobcat (governance dc3b2aa): the pinned `UCred` API, named in-process readers, caller versus target, rollout evidence, the context grant's status, the residual worded as a limit.
- Revision 2 answers the round-2 verdicts at f5c25d4:
  - second reviewer, spoonbill (governance b2bb85d): the ancestry cache is dropped, because a reparented cached ancestor defeats every hit check. Every tokenless request now walks to the boundary, and the regression for that chain is carried into phase 2. Implicit HEAD is gated with its GET.
  - ocelot's notes: a numeric global cap (4,096 boundary-index entries), and an O(1) boundary lookup per hop with no roster scan.
- It recommends option C with one owned session per incarnation. That rule is RULED by the prime (08 Nov). Send-as is gated first by the prime's requirement.
- The issuer is secured first, then operations are gated in order: send-as, inbox-claim, controls, role-and-holder, outputs, registration and adoption changes, activity, attribution.
- PRD 003's q-02 to q-04 are open, prime-owned dependencies; nothing in the design depends on their answers.
- Pending: security and reviewer verdicts at this head.

## AC3: cost

Phase 1 adds no daemon runtime cost. The only daemon source change is a `#[cfg(test)]` module declaration: no loop, poll, timer, hook or launch site, so `spawn_sites.rs` and the live spawn readings are unchanged by construction.

**The sensor's own price**, measured on this machine (round 2, ocelot P163-F002):
- **The recipe.** `just route-inventory-check` compiles the daemon's lib tests under `nice -n 19` and (macOS) `taskpolicy -b`, then runs them nice-only. It is the same split `just shared-build` uses, without that recipe's source refresh, which only a shared target needs and which recompiled the daemon on every run (2 min 54 s even when warm).
- **Cold** (first run, or after a daemon source change): one compile of `pij-daemon`'s lib tests, about 3 minutes at background priority. It is the same artifact `rust-check` and CI build.
- **Warm:** about 2 s end to end. The six tests themselves take 0.14 s, including the probe: 59 paths × 7 methods = 413 sequential local calls to an isolated test router, once.
- **Full gate:** `harness checks` and `just self-check` run the probe once. `rust-check`'s workspace tests skip the probe test (`--skip`), so it doesn't run twice. CI's nextest workspace run still includes it.
- **Bounds:** a 5 s timeout per call and 60 s for the whole enumeration, so a stuck handler fails the stage rather than stalling the gate.

**The workshop's design**, per request: one SHA-256 and a constant-time compare for token requests. A tokenless request costs one peer-credential read, a start re-read and a walk of at most 16 hops, each an in-process parent read, a start read and one hash lookup in the boundary index (global cap 4,096). Nothing launches per tick per seat.

## AC4: docs and states

- `docs/how/pij-rs-api.md` has a new "Seat authority" section pointing at the inventory, the sensor and the workshop.
- `docs/difficulties.md` records F88 as D-221-1, open until phase 2's gates ship.
- The plan's implementation summary states phases 2 and later as follow-up waves.
- No new verbs or refusals.
