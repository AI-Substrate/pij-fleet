# Plan 223 phase A: design note

For pm-001's scope check before the bulk of the code (accepted 08 Nov). Phase A ships as two count-only PRs (c8c592bf): **A1, the daemon** (AC1 to AC5, AC7, cost and docs), and **A2, the clients** (AC6). Nothing is refused except issuance. Facts are at main ed9a3d7c.

## Store (one migration, main's highest + 1 at push time, likely 0045)

- **`seat_nonces`**
  - Columns: `spawn_id TEXT PRIMARY KEY`, `seat TEXT NOT NULL` (the intended seat), `harness TEXT NOT NULL`, `digest BLOB NOT NULL` (SHA-256), `issued_at INTEGER`, `expires_at INTEGER`, `consumed_at INTEGER NULL`.
  - Written by `launch_seat_locked` (spawn and revive share it) before the window opens.
  - Expired rows are deleted when the same spawn id is next touched, and at daemon start. No sweep.
- **`seat_capabilities`**: one row per live incarnation.
  - Columns: `seat TEXT PRIMARY KEY`, `owner_pid INTEGER`, `owner_start TEXT`, `boundary_pid INTEGER`, `boundary_start TEXT`, `session TEXT NULL`, `digest BLOB NOT NULL`, `prev_digest BLOB NULL` (for two-phase rotation), `issued_at INTEGER`, `via TEXT` (`nonce` or `peer`).
  - The owner is the process that bootstrapped the incarnation (Copilot: the extension host). The boundary is the harness process (`seats.pid` and `proc_start`).
- **One transaction for issuance.** A new store method, `register_with_capability`, does the nonce consume (if any), the existing seat upsert, and the capability insert or replace together. Today the seat upsert (`Registry::put_reporting`, `crates/store/src/registry.rs:414`) and the grant are separate, and that gap is what this closes.
- **Revocation.** Tombstoning (`tombstone_if_unchanged`, `registry.rs:452`) and supersede delete the capability row in the same transaction.
- **SCHEMA_VERSION** follows the migration number.

## Daemon (`crates/daemon`)

| File | Change |
|---|---|
| `src/capability.rs` (new) | Pure parts: mint (32 OS-random bytes), the SHA-256 digest, constant-time compare, the in-memory digest-to-seat map and the boundary index (`(pid, start)` to live seats, capped at 4,096, event-maintained). Unit tests. |
| `src/http/provenance.rs` (new) | `CallerProof` (`Token(seat)`, `Peer(seat)`, `Neither`), computed per request from the `x-pij-seat-capability` header, or from the Unix peer (connect info), plus the fresh boundary walk. Pure walk over an injected reader, so the H to M to S to P regression is a unit test. Refusal codes `E-RS-SEAT-PROVENANCE`, `E-RS-SEAT-INDEX-FULL` and `E-RS-SEAT-AMBIGUOUS` are defined here. |
| `src/lib.rs` | Bind `<state-dir>/daemon.sock` (directory 0700, socket 0600, a stale socket removed at boot) and serve the same router with `into_make_service_with_connect_info::<UdsPeer>()`. `UdsPeer` reads `peer_cred()` (uid, pid) and `observed_start` at accept. TCP is unchanged. |
| `src/http/mod.rs` `register` | Issuance. A token is minted only when (a) a valid nonce is presented over the socket and the peer's nearest boundary is the spawned pane's process, or (b) the socket peer's nearest boundary is the claimed harness and owns no other live session, or (c) the current token is presented (rotation). Otherwise the seat registers as today, with no token and no context grant. `E-RS-SPAWN-NONCE-USED` plus an anomaly for a nonce race. |
| `src/reported_context.rs` | The context grant is folded in: `/v1/activity`'s `context_token` is checked against the capability digest. The plaintext map and the public-fact reissue path (`audit_grant_reissue`) are retired. |
| `src/http/mod.rs` `launch_seat_locked`, `crates/harnesses/src/spawn.rs` | Mint the nonce and pass it as `PIJ_SPAWN_NONCE` next to `PIJ_SPAWN_ID` (Codex: its shell env too). The nonce row is written before the window opens. |
| `src/http/identity.rs` `resolve_seat`, and the handlers that don't use it (send `from`, native inbox N, FYI B) | Count each operation by class. `resolve_seat` gains an operation-class argument at each call site (15 modules), so counting follows the operation and the argv leaf, not the URI. Nothing is refused. |
| `src/http/mod.rs` `Health` | `capabilities`: bounded counters (8 classes × {token, peer, neither}) plus `nonce_first_registrations`, modelled on `AttemptCounts`. No per-seat rows. |
| `crates/harnesses/src/proc.rs` | A Linux in-process parent read (`/proc/<pid>/stat` field 4) beside the macOS `parent_and_name`. |
| `crates/testkit/tests/spawn_sites.rs` | No new launch sites. |

## Clients (AC6)

| Client | Plan |
|---|---|
| Copilot native (`.copilot/extensions/pij/store.mjs`) | A `node:http` transport with `socketPath`, using built-ins only (C25-safe, `node --test`), falling back to TCP. The token comes from the register response (it already keeps `context_token`), held in memory and sent as `x-pij-seat-capability` on every call. |
| OMP and Pi (`.omp/extensions/pij/adapters/daemon-http.ts`) | The same: `node:http` over the socket, and the token from the register response sent on every call. The Pi adapter makes no daemon calls itself. |
| `pij` TS shim (`.omp/extensions/pij/cli.ts`, through daemon-http) | The socket, no token: it acts through peer credentials. |
| `pij-rs` (`crates/cli/src/client.rs`) | A Unix transport using `hyper::client::conn::http1` over a tokio `UnixStream` (hyper is already in the lockfile with its client features; no new crate), with TCP as the fallback. No token: peer credentials. |
| Claude, Codex and Copilot hooks and status lines | They run `pij-rs`, so they get the socket through it. Before relying on it, a private-daemon check confirms a Claude hook process descends from the Claude process (they are `type: command` hooks run by claude itself). If not, I stop and tell pm-001. |

## pm-001's points (08 Nov)

1. **After a bounce**, the in-memory digest map and the boundary index are rebuilt from one store read of live rows. A test shows a live seat's token still works after a restart.
2. **The stale socket at boot.** It is unlinked only after a connect to it fails. A live daemon's socket answers the connect, so it is never removed (the existing single-instance guard runs first as well).
3. **Observed-dead tombstones revoke too**, not only owner-close and supersede: every tombstone write deletes the capability row in its transaction.
4. **Short socket paths in tests**, since macOS caps `sun_path` at 104 bytes; test state dirs use a short `/tmp/pij-*` root.
5. **Plan 221's evidence** says "pass-with-notes", not "pass" (squirrel's LOW on #166).

## Order of commits (A1 then A2)

1. AC1: inventory notes and an operation class per entry.
2. AC2: the migration, store methods and issuer.
3. AC3 and AC4: the socket, peer credentials, the walk, the index and ownership.
4. AC5: secret-handling checks (no token in env, argv, logs, `/health`, roster or spine; a test greps responses and events).
5. AC7: counters.
6. AC6: clients.
7. Docs and states.

Commits 1 to 5 and 7 are A1; commit 6 is A2.

macOS clippy and tests run locally before every push.

## Scope risks to flag now

- **Size.** About 15 handler modules gain a class argument, and three clients change. I'll keep each client change to a transport plus a header.
- **Reviewer load:** settled by the A1 and A2 split.
- **q-02 to q-04.** Nothing here touches them.
