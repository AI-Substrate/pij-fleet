# Plan 223 phase A1 evidence (the daemon)

Phase A ships as two count-only PRs: A1 (the daemon, this file) and A2 (the clients), stacked. **Deploy together; never bounce on A1 alone.** With A1 alone, today's clients get no capability, so native Copilot seats' context-size reports would stop until A2.

## AC1: inventory

- Squirrel's three LOW notes on #163 (governance 7e537dc) are fixed in `harness/scripts/route-inventory.json`:
  - registration's guard describes its ordinary branch too (`N`, `R`);
  - `POST /v1/bg` grants by owner and parent;
  - compact-self's send aliases reach the parent-authorized controls.
- Every seat-authority entry lists `operations`: the classes it reaches, its own class included. Shared handlers list each one (send and shim send reach controls; compact-self reaches send-as; directory and orchestration reach attribution).
- The sensor checks that `operations` is a non-empty, known, unrepeated list including the entry's class, and that every implicit HEAD carries its GET's classification exactly. Both have tests.

## AC2: the issuer

- **Migration 0046** (`seat_nonces`, `seat_capabilities`, SCHEMA_VERSION 46; 0045 is held by #184).
- **The nonce:**
  - Spawn and revive mint a 32-byte nonce, store its SHA-256 with spawn, seat, harness and a 15-minute expiry (`NONCE_TTL`), and write it to a 0600 `create_new` file beside the spawn's status file.
  - `pij-rs __spawn-child --nonce-file` reads and unlinks the file and sets `PIJ_SPAWN_NONCE` on the harness only, so it is never in argv (Codex's shell environment doesn't get it either).
  - An unread file is deleted when its spawn's launch fails, when its expired row is dropped (at daemon start, or when the expired nonce is presented), or when the same spawn id is reused. No sweep.
- **Issuance is one store transaction** with the seat upsert. It happens on:
  - a nonce matching spawn, seat and expiry, arriving over the socket from the spawned process;
  - the socket peer path (AC3);
  - the current token with an explicit `rotate`, which is two-phase: the old digest stays valid until the new token is first presented.

  A same-incarnation re-registration presenting its token gets the same token back and writes nothing.
- **Public facts only:** the seat registers as before, with no token and no context grant (F88 row 2 closed). The context grant is folded into the capability, and `/v1/activity`'s `context_token` is checked against the capability digest.
- **Refusals:** `E-RS-SPAWN-NONCE-USED` (409, plus a `capability.nonce-reused` event that names no secret), `E-RS-SPAWN-NONCE-INVALID`, `E-RS-SEAT-CAPABILITY`. A refused proof commits nothing.
- **Revocation and restarts:** every tombstone (owner close, supersede, observed dead, death sweep) deletes the capability in its transaction. Digests persist, and the digest map and boundary index are rebuilt from one store read at start. A test shows a live seat's token still works after a restart.

## AC3: the socket and the walk

- **The socket:** `<state-dir>/daemon.sock` (socket 0600, state directory 0700) serves the same router. A stale socket is unlinked only after a connect to it fails. A path over macOS's 104-byte limit leaves TCP serving alone and logs why.
- **The peer:** read once at accept with `peer_cred` (uid, pid) and an in-process start read. A foreign uid, a missing pid or an error proves nothing; it never falls back to body identifiers.
- **The walk:** a pure function in `crates/daemon/src/http/provenance.rs`. It reads parent and start in-process (`parent_and_name`; on Linux a new `/proc/<pid>/stat` field-4 read) and goes at most 16 hops to the nearest registered `(pid, start)` boundary. Pid 1, launchd, a tmux server, an unreadable or reused ancestor, or 16 hops end it with `E-RS-SEAT-PROVENANCE`. **No ancestry cache.** The H→M→S→P regression (M dies, S reparented to 1) refuses P.
- **The boundary index:** `(pid, start)` to live seats and sessions, rebuilt at start, kept current by a synchronous observer on the registry's event bus (every registry write passes through it), capped at 4,096 (`E-RS-SEAT-INDEX-FULL`, 503, nothing evicted).
- **Adopt over the socket:** the explicit-harness-session check uses the socket peer instead of `lsof`. The TCP path is unchanged. Tests cover a descending and a foreign peer.

## AC4: one owned session

- A boundary hosting two or more live sessions proves nothing tokenless, with the ruled text: `E-RS-SEAT-AMBIGUOUS: this Copilot process hosts more than one live session; start a fresh seat (pij-rs revive --fresh), don't /new` (the harness named). No capability is issued that way, and the request counts as neither.
- The owner recorded is the bootstrapping peer (Copilot: the extension host). Ownership moves only by token, or after the owner is observed dead.

## AC5: secrets

- **Where tokens live:** only in the owner's memory and its registration response. Tests show a token never appears in `/health`, `/v1/seats`, `/v1/cards` or spine events. Secrets are held in redacted types; no test captures stderr.
- **The nonce file** is still readable by the same user, like the environment it replaces (pm-001, 08 Nov). It is created 0600 and unlinked once read.
- **The non-loopback listener** refuses any request carrying `x-pij-seat-capability` (403, `E-RS-SEAT-PROVENANCE`).
- **A socket or probe failure** counts as neither.

## AC7: counters

`/health` `capabilities.classes` holds 8 classes × {token, peer, neither}, plus `nonce_first_registrations`.
- **Counted by operation:** `resolve_seat` takes the class at all 24 call sites, and send, the native inbox, ack, heartbeat, FYI claim, activity, register, adopt, spawn and revive count explicitly.
- Shared handlers count by argv leaf, implicit HEAD counts with its GET, and machine-public reads (governance `list`, `show` and `status` leaves among them) are not counted.
- **Nothing is refused** for a missing capability.

## AC9: cost (so far)

- No new process launch: `spawn_sites.rs` passes.
- A tokenless request's walk measured about 23 µs on this machine (one hop, real kernel reads).
- A token request is one SHA-256, a constant-time compare and a map lookup.
- Registration adds one primary-key read; daemon start adds one delete and one select.
- **Still owed:** the live before-and-after readings at the A1 plus A2 deploy (`pij-rs --json ping | jq .data.spawns`, `just spawn-rate --seconds 20`, daemon CPU at rest).

## Gates

Local macOS runs, niced, through `just shared-build` with `-j 4` and `--test-threads=4`. The results are in the PR body.
