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

- **Migration 0047** (`seat_nonces`, `seat_capabilities`, SCHEMA_VERSION 47; 0045 is held by #184 and 0046 by #189).
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

## Review round 2 (#183)

Every fix has a regression, and each regression was run against a mutant that reintroduces its defect. The mutant was applied, the test failed at the asserted defect, and the code was restored.

| Finding | Fix | Regression | Mutant (killed) |
|---|---|---|---|
| S183-001 / D3 | A registration without proof (neither the seat's capability nor its spawn's nonce) may not move a live owner's capability. That covers rebinding its process or session, bootstrapping a new owner over the socket, and retiring a seat a live owner holds (a supersede, a spawn's predecessor). It is refused `E-RS-SEAT-OWNER-LIVE`, retryable (409), with nothing written. Owner and boundary are the capability row's own fields. The escape is `pij close <seat>` | `a_proofless_move_of_a_live_owners_seat_commits_nothing`; reload: `a_socket_peer_bootstraps_a_capability_and_owns_it`; escape: `a_hung_owner_is_escaped_by_closing_the_seat` | guard result ignored: all three fail |
| S183-002 / D1 | One retirement path: the new capability's first presentation by any carrier (a header at the door, a held registration, or `context_token`) commits the store-conditional confirm before the request goes on. A failed write fails the request | `every_presentation_of_the_new_capability_retires_the_old_one` (held registration and header operation; the old token is then refused for reporting and for rotation); `a_failed_retirement_fails_the_presentation` | header path skips the retirement: fails at "held registration: reporting"; confirm error ignored: fails |
| D2 | The in-memory retirement only applies if the confirmed digest is still current (generation) | `a_paused_confirm_never_retires_the_next_generations_previous` | generation filter removed: fails |
| S183-003 | Locality is checked at every capability ingress: the header, `/v1/activity`'s `context_token`, and a registration's `spawn_nonce`. Each is refused 403 before anything is read or written | `the_remote_listener_refuses_every_capability_carrier_before_any_mutation` | activity body check disabled: fails |
| D4 | A root (pid 1, launchd, tmux) is refused before any boundary or target match | `a_root_is_never_a_boundary_a_target_or_a_peer` | root check moved after the match: fails |
| D5 | The spawn's pane process is recorded on the nonce row after launch. The claimed process must be it or descend from it | `a_nonce_binds_only_a_process_inside_its_spawns_pane` (the nonce is not consumed) | pane check skipped: issued to the unrelated process |
| D6 | `reconcile_descriptors` revokes the alias's capability (and the survivor's, if its process changed) in its transaction | `reconcile_revokes_the_alias_capability_with_its_tombstone` (store) | revoke removed: fails |
| D7 | Predecessors retire inside the issuing transaction (`put_issuing(.., retiring)`). Every event in it is published in order (`publish_registry_batch`) | store contract section 6 (SQLite and fake); `a_failed_issuing_write_leaves_the_predecessor_and_its_capability` (HTTP) | separate retire writes in SQLite, the fake and the daemon: each fails |
| D8 / P183-F002 | One `SeatAuthority` (capabilities, boundaries, reader) is injected into every `RegistrationService` (the routes, Claude binder, Codex binder). Defaults exist only under `#[cfg(test)]`. The cap is checked and reserved in one step and held until the write commits (`Admission`). 4,096 is an admission limit | `an_admission_holds_its_slot_until_dropped`, `a_binder_shares_the_cap_and_the_capability_map`, `two_services_racing_for_the_last_slot_admit_exactly_one` | check without reservation: both racers admitted; binder's private authority: admitted past the cap |
| D9 | `directory memory` and `directory routes` count inside the leaf once read or write is decided; reads are uncounted | `memory_and_routes_count_their_writes_and_never_their_reads` | leaf-level class restored: the memory read counts |
| P183-F001 | `hosted_by` and `hosts_other` decide nobody, one or many from at most two entries, copying nothing | `hosted_lookups_copy_nothing_out_of_the_index` (256 sessions of 64 KiB) | copy the hosted map: fails |

**Close-then-bootstrap (ruled by pm-001 and the prime after round 2):**
- **The gap:** a proofless close "as the victim", followed by a peer bootstrap from the attacker's own process, would gain the victim's identity. It is S183-001 by another route.
- **The mark:** a tombstone that revokes a capability, proven neither by the seat's own capability nor by its owner's observed death, writes a mark with its reason (`seat_reissue_marks`, migration 0047) in the revoking transaction.
- **While marked:** a peer bootstrap is refused `E-RS-SEAT-REISSUE-NONCE` (403, nothing written), in the store's issuing transaction. Only a spawn or revive nonce, or the current capability, re-issues the seat, and either clears the mark.
- **Reasons:** a close's `--reason` may not read as a proven one. A close presenting the target's capability is recorded as proven.
- **The escape is close then revive**, and the `E-RS-SEAT-OWNER-LIVE` text now names both. Revive does not refuse a closed seat whose pane is still present: it launches in a new pane.
- **Regressions:** `a_proofless_close_then_bootstrap_is_refused_and_revive_reissues` (HTTP) and capability contract section 7 (SQLite and the fake). The reload path (owner observed dead, then a peer bootstrap) is `a_socket_peer_bootstraps_a_capability_and_owns_it`.
- **Mutants, killed:** with the fake's mark check disabled, the attacker is issued. With SQLite's in-transaction check disabled, the contract fails with "issued where a refusal was due".

**Clients and the retryable refusal (pm-001's reload condition):**
- **Native Copilot:** `.copilot/extensions/pij/store.mjs:601-608` maps a 409 `pij register` refusal with `details.retryable: true` to a retryable `NativeError`. `extension.mjs:224-256` then retries the whole registration with exponential backoff (`INITIAL_RETRY_MS` doubling to `MAX_RETRY_MS`) until it succeeds. Today's Copilot client registers over TCP and never holds a capability, so it meets this refusal only once A2 is deployed.
- **OMP:** it does not retry. `rust-runtime.ts:309` (`registerIdentity`) rethrows any refusal except a retired seat, and `index.ts:466-471` marks the boot failed. OMP's owner is its own process, so a reload in the same process is never this case. A2 should add a retry if OMP ever re-registers from a new process while the old one lives.

**Cost:** a launch adds one tmux pane read and one in-process start read (the pane record). A registration adds at most one extra capability-row read per retiring seat and one walk for a socket bootstrap. Nothing runs per tick.

## Gates

Local macOS runs, niced, through `just shared-build` with `-j 4` and `--test-threads=4`. The results are in the PR body.
