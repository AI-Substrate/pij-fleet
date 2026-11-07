# Workshop 001: capability bootstrap (plan 221, PRD 003 req-10, F88)

**Decision wanted:** how each seat incarnation gets a private capability that every seat-authority request must present, so phase 2 can gate the routes in `harness/scripts/route-inventory.json` on it. Security and reviewer verdicts on this file's commit come before phase 2 is planned (bp-0002).

**Recommendation:** option C, with session ownership made explicit.
- **Bootstrap, once per incarnation:** a single-use spawn nonce for spawned and revived seats, or Unix-socket peer credentials checked against the nearest harness boundary. Never roster identifiers.
- **The per-request control:** a per-incarnation token, stored only as a digest, held in the owning process's memory. One hash compare per request.
- **Short-lived clients** (the shim and hooks) act through peer-credential ancestry, but only where the nearest harness boundary owns exactly one live session. Ambiguity is refused.
- **The issuer is secured first**, as phase 2's prerequisite.
- **Then phase 2 gates the operations in this order:** send-as, inbox-claim, controls, role-and-holder, outputs, registration and adoption changes, activity, attribution.

**Revision 1** answers ocelot (P163-F001, governance b78df40), squirrel (governance f1d74f3) and bobcat (governance dc3b2aa) at bf89eed. It adds in-process ancestry with a strict no-launch refusal and a cache with an explicit lifetime, session ownership for shared Copilot hosts, issuer-first, nonce and incarnation transactions, secret and transport rules, operation-level gating, and PRD 003's open dependencies.

## What exists today

Facts at main `2cc2ac3`, with sources:

- **One credential for everything.** The daemon listens on TCP only (`crates/daemon/src/lib.rs:1140-1150`; an optional remote listener serves the same router). Every client reads the one daemon key from `<state-dir>/daemon.key`: 256 random bits in a 0600 file, compared in constant time (`crates/daemon/src/auth.rs:83-109`, `http/auth.rs:130-139`). No peer-credential code exists anywhere.
- **Seats identify themselves with public facts.** Registration sends the seat, harness session, pid, process start, pane and spawn id (`http/types.rs:6`). The daemon observes the claimed pid and start on the host (`registration.rs:334-419`), but that proves the process exists, not that it sent the request. F88 lists 43 seat-authority pairs whose guard reduces to these facts.
- **Spawn passes identifiers, not secrets.** A spawned harness gets `PIJ_SESSION_ID=<seat>` and `PIJ_SPAWN_ID=<spawn>` on its command line, run by the tmux server in a new window (`crates/harnesses/src/spawn.rs:134-145`, `http/mod.rs:1537`). The daemon never forks the seat, so it can't hand the seat a file descriptor.
- **Two secrets already exist, and one is the model.**
  - The background emit token: 32 random bytes per job, delivered in the runner's environment (`PIJ_BG_TOKEN`), stored only as a SHA-256 digest, checked by hashing and comparing in constant time (`crates/daemon/src/background.rs:374-423, 509-519`). Nothing reissues it from a seat's identifiers.
  - The context grant: 32 random bytes per seat incarnation, held in memory in plaintext, checked on `/v1/activity` (`crates/daemon/src/reported_context.rs:84-126`). It is re-returned to anyone who repeats the native registration with the right public facts (F88 row 2; the reissue is audited, `http/mod.rs:1164-1177`). That is the failure the capability must not repeat.
- **Clients.**
  - Long-lived, holding the daemon key in memory: the native Copilot extension (`.copilot/extensions/pij/store.mjs:441-485`) and the OMP and Pi extensions (`.omp/extensions/pij/adapters/daemon-http.ts:55-121`).
  - Short-lived: the `pij` shim (`.omp/extensions/pij/cli.ts`, which derives its caller by registering or reading the environment, `adapters/generation-router.ts:374-461`), `pij-rs` (`crates/cli/src/client.rs:138-153`), and the Claude, Codex and Copilot hooks and status lines (`crates/cli/src/main.rs:25-33, 947-953`). An agent's bash tool runs as a subprocess of its harness and inherits its environment.

- **One Copilot process can host several live sessions.** Copilot keeps every session's extension host alive after `/new` and the `--resume` picker. Each extension host is its own process, a child of the `copilot` process (`.copilot/extensions/pij/extension.mjs:143-168`). The daemon ranks them by plan 168's precedence report (`store.mjs:61-69`), an observation, not authentication. An agent's bash tools are children of the `copilot` process, not of their session's extension host, so a tool's ancestry reaches a host that all its sessions share.

## Threat model and its honest limit

The attacker in F88 is **a local caller holding the daemon key**: in practice another seat, or a script it runs, acting through pij's API as the same OS user. The control raises impersonation from "read the public roster" to "steal another process's private state": its memory, its environment (`ps -E` on macOS, `/proc/<pid>/environ` on Linux), its files, or running code inside its process tree (for example typing into its tmux pane). **A same-user attacker can do all of those**, and no design here stops it; that needs separate OS users per seat, which is out of scope. The capability makes impersonation a deliberate OS-level act rather than an API call that looks like normal use. **That is a stated limit, not a detector:** nothing here instruments memory or environment reads, so such an act is not guaranteed to be seen. Every option below is judged against that bar.

## The options

### A. A per-incarnation secret minted by the daemon, delivered privately

The daemon mints 32 random bytes per incarnation, stores only the SHA-256 digest, and the seat presents the secret on every seat-authority request.

- **Spawned seats.** An inherited fd is impossible: the tmux server forks the pane, not the daemon. A 0600 file under a seat-private directory, or the spawn environment (`tmux new-window -e`, or the existing command-line variables), are both possible. Both are readable by the same user, and the environment is inherited by every subprocess.
- **Revived seats:** the same as spawned (revive relaunches through spawn).
- **Adopted and hand-started seats:** nothing to deliver it through. The daemon didn't start the process, so the secret could only come back in a response to a request that is itself unauthenticated, which is the F88 reissue again. **Option A has no answer here on its own.**
- **Clients.** Long-lived extensions hold it in memory. The shim and hooks need it from the environment or a file, so every subprocess of the seat (every bash tool) can present it. That is the right authority (the seat's own tools act as the seat), but it widens exposure to anything that can read an environment.
- **Rotation and death.** A new secret per incarnation; the digest is cleared on tombstone and on supersede. Re-registration proves the current secret.
- **Daemon restart.** Lost unless the digest is persisted. Persisting it in the store keeps clients working across a bounce.
- **Per-request cost:** one SHA-256 of 32 bytes and a constant-time compare. No launch.
- **Residual:** any same-user process can read the spawn environment or file. Hand-started seats can't get one at all.

### B. OS peer credentials on a Unix socket

The daemon adds a Unix socket listener (`<state-dir>/daemon.sock`, 0600, in a 0700 directory) serving the same router. For each connection it reads the peer's pid and uid (Linux `SO_PEERCRED`; macOS `LOCAL_PEERPID` and `getpeereid`; tokio's `UnixStream::peer_cred` covers both). It then checks that the pid is the registered incarnation (pid plus process start) or a descendant of it.

- **Spawned, revived, adopted and hand-started seats all work the same way**, with nothing to deliver: the evidence is which process connected.
- **Clients.** All of them switch from TCP to the socket. Subprocesses (bash tools, hooks) are descendants of the harness, so they pass as the seat. A process outside the seat's tree, even with the daemon key, does not.
- **Rotation and death.** Nothing to rotate. The registered pid and start die with the incarnation.
- **Per-request cost.** A connecting pid that isn't the harness itself needs an ancestry walk. With the strict in-process path (see [Peer credentials and the ancestry cache](#peer-credentials-and-the-ancestry-cache-contract-c-with-ocelots-p163-f001)), that is O(D) kernel reads and no launch, on macOS as on Linux. So cost alone doesn't rule B out. It does put a lineage walk on every request, including the long-lived clients' per-tick heartbeat, inbox and activity calls, where a token would be one hash compare.
- **Residual.** A process that runs inside the victim's tree (a typed command in its pane) passes, and so does a pid reused after the victim died, unless process start is checked. F88 also notes peer credentials can't tell apart two sessions sharing one host process. req-10 itself calls peer credentials bootstrap evidence, not the whole control.

### C. Both: peer credentials (or a spawn nonce) as bootstrap evidence, the token as the per-request control

The parts below are contracts for phase 2: design, not code.

#### Session ownership (squirrel's HIGH: shared hosts)

- **A seat incarnation is one owned session.** Its **owner** is the process that bootstrapped it:
  - for Copilot, that session's extension host;
  - for OMP and Pi, the harness process;
  - for Claude and Codex, the harness process.

  The owner holds the token in memory. Ownership moves only by presenting the current token, or after the owner is observed dead (its start re-read fails or differs). Two live candidate owners for one session make it **ambiguous**, and every claim on it is refused by name (`E-RS-SEAT-AMBIGUOUS`). Public session ids and precedence observations never decide ownership.
- **The nearest harness boundary.** For a connecting pid, it is the nearest ancestor (or the pid itself) that is a registered harness process, checked by pid plus start.
- **Tokenless authority needs exactly one live owned session at that boundary.** A shim or hook call that reaches a Copilot process hosting two or more live registered sessions is refused `E-RS-SEAT-AMBIGUOUS`: a caller in session A could otherwise present session B's identifiers and pass. The extension hosts' own requests are unaffected, because each carries its own session's token from its own process.
- **DECIDED (pm-001, 08 Nov): keep the one-owned-session rule.** A pij seat is one session per harness process. pij-spawned seats are that by construction. `/new` or `--resume` in a pij Copilot seat leaves the old session's host alive, so that seat's tools can't act tokenlessly until the extra sessions end. The refusal names the fix: `E-RS-SEAT-AMBIGUOUS: this Copilot process hosts more than one live session; start a fresh seat (pij-rs revive --fresh), don't /new`.
  - Why: fleet practice already replaces seats with `revive --fresh` rather than `/new`, compaction keeps the session, and refusing ambiguous ownership is what closes squirrel's HIGH. This is the dedicated process boundary per incarnation that squirrel offered as one way out. The prime may override.
- **The alternative, recorded and not chosen:** keep `/new` working and wait for a private session-host attachment, meaning a per-session channel from a session's extension host to that session's tools only. No harness offers one today, and a per-session secret in the tool environment would break the secret-handling rule below, so waiting would leave the hole open with no date. If Copilot gains such a channel, it can replace the one-session rule for Copilot.

#### The issuer first (contract a)

- **Issuance, recovery, rebinding and supersession** of a capability, and of the context grant once it is folded into the capability, are authorized only by bootstrap evidence or the current token.
- **Today's context grant is not the capability.** It is reissued from public facts (`crates/daemon/src/reported_context.rs:84-126`, F88 row 2). It becomes part of the capability only when that reissue path is closed by this secured issuer, and until then it is no evidence of anything. Legacy roster claims never authorize them, **not even in the count-only rollout phase**.
- A TCP registration carrying only public facts still registers as today, so old clients keep working, but it gets no token and no context grant. That closes F88 row 2.
- Securing the issuer is phase 2's **prerequisite**, shipped before any route class is counted or gated, not a late gate class. Phase 2a's "nothing refused" means legacy requests stay compatible. It never means a privileged token is issued, reissued or rotated from public facts. Issuance, nonce consumption and incarnation binding, revocation, and this guard on every listener (Unix socket and TCP alike) all uphold option C before the first effect gate.

#### Nonce and incarnation transactions (contract b)

- **Independent secrets.** The nonce and the token are each 32 bytes from the OS random source, independent of each other.
- **What a nonce is bound to.** Its digest is stored with the daemon-authorized spawn or revive: spawn id, intended seat, harness, issue time and expiry.
- **Consumption is one store transaction.** Registration with a nonce, inside one transaction:
  1. finds an unconsumed, unexpired nonce row whose spawn and intended seat match the claim;
  2. marks it consumed;
  3. creates or binds the incarnation;
  4. stores the new token's digest.

  A failure anywhere commits nothing. Two concurrent registrations serialize; the loser is refused `E-RS-SPAWN-NONCE-USED` and an anomaly is recorded. A duplicate claim is worth a look, but it proves neither theft nor innocence.
- **The nonce is also peer-checked.** A nonce registration must arrive over the Unix socket from a process whose nearest harness boundary is the spawned pane's process.
- **Abandoned launches.** A nonce expires with its spawn's registration window. Expired rows are dropped when that spawn is next touched, or at daemon start. **No sweep.**
- **Daemon restarts.** Nonce and token digests are persisted, so a seat registering after a bounce, or one already holding a token, carries on.
- **Rotation, two-phase.** Re-registration presenting the current token may get a new one. The old digest stays valid until the new token is first presented, so a lost response can't strand the client. The old one is then dropped.
- **Revocation.** Tombstone, supersede or an observed-dead owner deletes the incarnation's digests in the transaction that records the change.
- **Lost token.** Only the owner holds the token, so losing it means the owner process restarted, and that is a new incarnation: it bootstraps again, through peer credentials or a fresh nonce. Digests are never "recovered" into plaintext.

#### Peer credentials and the ancestry cache (contract c, with ocelot's P163-F001)

- **Credentials come from the accepted connection, never from headers, the body or a claimed pid.** The API is pinned: `tokio::net::UnixStream::peer_cred()` on the accepted stream, giving `UCred` uid plus pid on Linux (`SO_PEERCRED`) and macOS (`LOCAL_PEERCRED` plus `LOCAL_PEERPID`). They are read once at accept. A missing pid, an error, or a uid other than the daemon's is a failure that refuses every seat-authority request on that connection. It never falls back to body identifiers. The peer's start is read in-process immediately, and the (pid, start) pair is bound to the connection.
- **Every request re-reads the peer's start.** A connection inherited by a child, passed with `SCM_RIGHTS`, or pooled keeps the original connector's credentials. If that process has died, its start re-read fails and the request is refused. If it is alive and handed its connection on, it chose to delegate its own authority. Clients must not pool connections across sessions.
- **Reads are in-process, never launched.** The readers are named:
  - **Parent:** `pij_harnesses::proc::parent_and_name` on macOS (`proc_pidinfo` short BSD info, `crates/harnesses/src/proc.rs:347-355`). On Linux, phase 2 adds the same read from field 4 of `/proc/<pid>/stat`.
  - **Start:** `pij_harnesses::proc::observed_start`, which reads the kernel start (`kernel_start`, macOS BSD info or Linux `/proc/<pid>/stat`, `proc.rs:161-177`). It launches nothing, and returns `None`, failing closed, when the kernel won't say (`proc.rs:258-271`). Plan 218's Claude socket peer check already uses it this way.
  - Never the `ProcLiveness` wrapper, which can fall back to `ps`.
- **Unreadable evidence is refused, never escalated.** A read that doesn't answer (the process is gone, or the kernel won't say) refuses the request by name (`E-RS-SEAT-PROVENANCE`). It never falls back to `ps` the way the `ProcLiveness` wrapper can, so the recurring path is strictly launch-free. C27's argv check, which does launch `ps`, stays in bootstrap: registration and adoption, once per incarnation.
- **The walk.** It starts at the peer pid and follows parents for at most D = 16 hops. It stops at the **nearest** registered harness boundary, whose start must match, or at the first cached ancestor.
  - It never continues past a harness boundary, so a sibling or a shared tmux server, login shell or launchd above it never validates.
  - Reaching pid 1, launchd or a tmux server without a boundary, or using up D, is a refusal.
  - The work is O(D) kernel reads.
- **How a fresh pid reaches the cache.** A new shim process names nothing, so the walk is what finds its ancestor: at each hop it looks up (pid, start) in the cache, and the first hit ends it. Its shell, or the harness, is usually one or two hops up.
- **Entries.** An entry is keyed by (ancestor pid, ancestor start) and maps to the boundary (harness pid and start) and the one live incarnation it owns. It is inserted only for ancestors that a walk proved sit below that boundary.
- **A hit proves only the current prefix.** A hit authorizes a peer only because that peer's own walk, read fresh on this request, reached the cached ancestor. The prefix from the peer up to the ancestor is current membership; the cache only saves the rest of the climb. A cached root never authorizes a sibling, or a process that has left its tree.
- **Each hit is re-checked.** A surviving ancestor alone never validates:
  - the ancestor's current start must equal the key's (pid reuse);
  - the boundary must still be alive, with the same start;
  - the incarnation must still be current: not tombstoned, not superseded, same session;
  - the boundary must still own exactly one live session (ambiguity).
- **Invalidation.**
  - Lazily, on a failed re-check: death, pid reuse, the incarnation ending, or ambiguity.
  - On events: tombstone, supersede, re-registration with a new incarnation, token rotation, a session change, or a second session registering under the boundary drops every entry indexed under that boundary or incarnation.
  - Changed ancestry, pid reuse and ancestor death fail the fresh prefix walk or the start re-check, and drop the entry.
  - Reparenting can't grant wrongly. A hit needs the peer's own walk to pass through the cached ancestor now, and a reparented process's walk no longer does.
- **Storage.** At most 64 entries per incarnation (least recently used goes first) and a global cap. Retirement is lazy and event-driven. **No new poll or sweep.**

#### Secret handling and transport (contract d)

- **Where tokens may live.** Only in the owning process's memory and in the registration response to it. Never in a subprocess's environment or argv, and never in diagnostics, logs, `/health`, the roster, cards or the spine.
- **The nonce** is the one secret in an environment. It is single-use, worthless once consumed, and peer-checked at use (above).
- **Fail closed.** A socket or probe failure never falls back to machine-key-plus-identifiers authority over TCP. During the count-only phase such a request is counted as "neither"; once its class is enforced, it is refused.
- **No capability over the network.** Seat-authority requests travel over the Unix socket, or carry the token on loopback TCP. The optional remote (non-loopback) listener never accepts a capability-bearing request: paired machines keep their three-route scope and alias attribution.

#### Gate operations, not URI names (contract e)

- **The unit of gating is the operation, not the route.** Shared handlers reach writes through other URIs: `/v1/outputs` with output argv, `/v1/claims` and `/v1/delivered` with claim or release argv, `/v1/shim/compact-self` with other send argv. Phase 2 gates each operation, wherever it is reached.
- **The caller is not the target.** The caller's token, or its peer credentials, proves only who is calling. A parent, PA or prime acting on another seat passes the existing authorization checks as itself and never needs that target's token.
- **Authorization checks stay.** The capability replaces only the input to the resolver: who is calling. The actor, parent, prime, holder, owner and recipient checks still apply, now to a proven caller.
- **Included explicitly:**
  - background-owner reads (`GET /v1/bg`, tail);
  - output answers;
  - dispatch ACK;
  - role-bearing spawn and the revive override;
  - context-grant recovery.
- **Intentionally machine-wide operations stay public:** the roster, events, cards and canonical reads.
- **The inventory follows.** It may grow a per-operation column in phase 2.

#### Cost

Budgets for phase 2; no syscall count or latency is proven here.

| Path | Cost | Launches |
|---|---|---|
| Long-lived token request | One SHA-256 of a fixed 32-byte token, a constant-time compare and a lookup in a warm digest-to-incarnation map. This is added authentication cost, not the route's whole cost. | none |
| Short-lived, cache hit | A constant number of peer-credential calls per connection, the walk up to the first cached ancestor, and the re-checks above. Not free. | none |
| Short-lived, cache miss | At most D parent-and-start reads plus a binding lookup. No whole-machine census. | none |
| Bootstrap and rotation | Nonce and token hashing, single-use nonce consumption, digest persistence and a map update. Any C27 argv check (`ps`) runs only here, once per incarnation or adoption. | bounded, per incarnation |

- **Nothing launches per request, so nothing launches per tick per seat.** That holds even where a status line or harness hook runs on the harness's own timer.
- **Lifecycle budgets phase 2 must fix:**
  - token-map load after a daemon bounce: one store read of live incarnations' digests;
  - unused nonces: they expire with the registration window;
  - the cache's capacity, as above.
- **Rollout counters** are bounded aggregates (operation class × outcome), read as interval deltas on routes that actually carried traffic. A zero "neither" on an unused route is not migration proof.
- **Phase 2's backpressure** states the measured rates and records any new launch site in `spawn_sites.rs`.

#### Residual for a daemon-key holder without the capability

- **Still reachable:** machine-public routes (the roster, events, cards, reads), ordinary spawning without a role, sidecar, hold and release, and the job-token hook with its own token. **This is authentication classification only** (machine-public, sidecar and system messages, the job-token hook). Whether a sidecar or system message may reach whom, and who is exempt from limits, are PRD 003's open q-02 to q-04, not decided here.
- **Needs an OS-level act:** impersonating a seat means reading the owner process's memory, winning the race for an unused spawn nonce (the real seat's late registration is refused and recorded as an anomaly, which proves neither theft nor innocence), or running code under the victim's harness boundary (for example typing into its pane).
- **Federation is unchanged:** a paired machine's key stays scoped to its three routes, with sends attributed to the machine alias.

## Why C

- **A alone** can't bootstrap hand-started or adopted seats without reissuing from public facts, and to serve the shim and hooks it puts a long-lived secret in every subprocess's environment.
- **B alone** is viable on cost with the strict in-process path above. It falls short on identity: it can't tell apart two sessions in one host, and it puts a lineage walk with a pid-reuse re-check on every per-tick call. req-10 calls peer credentials bootstrap evidence, not the whole control.
- **C** uses each mechanism where it's strong:
  - peer credentials (and the single-use nonce) as rare bootstrap evidence;
  - a hash compare per request for long-lived owners;
  - boundary-limited ancestry for short-lived clients, refused wherever the boundary is shared.

## Rollout (old clients during the change)

1. **Phase 2a, the issuer and transport.**
   - Add the Unix socket listener next to TCP, both serving one router.
   - Add the nonce, the token and session ownership, with issuance secured from day one (contract a).
   - Clients send the token and switch to the socket.
   - The daemon counts each seat-authority operation as token, peer-credential or neither, per class. Bounded counters in `/health`, read as deltas under traffic. No route is refused yet, apart from issuance.
2. **Phase 2b, enforce class by class**, in the order below. Each refusal is named (`E-RS-SEAT-CAPABILITY`) and tells an old client what to update. A class flips from count to refuse only on positive evidence:
   - **Positive execution:** every representative client that uses the class (the Copilot native, OMP and Pi extensions, the shim, `pij-rs`, and the Claude, Codex and Copilot hooks) completes at least one capability-proven operation of that class on the live fleet.
   - **Negative evidence:** a test shows the same operation without the capability is refused.
   - **A zero "neither" delta** over a working day, counted only for a class that carried traffic. A class nobody exercised proves nothing.
   - **Counted separately:** nonce-authenticated first registrations, so spawn bootstrap is visible apart from token and peer-credential traffic.
3. **TCP** stays for machine-public routes and paired machines. Seat-authority operations over TCP must carry the token, on loopback only.

## Operation classes in the order phase 2 should gate them

After the issuer (above), which is the prerequisite:

1. **send-as** (`/v1/send`, `/v1/shim/send`): the highest-impact impersonation (sending, FYI and role requests as another seat), and the one #152 round 4 showed still accepts an asserted session id.
2. **inbox-claim** (inbox, acks, heartbeat, FYI claim, dispatch ack): stealing or committing another seat's deliveries.
3. **controls** (close, the revive override, inbox release, background jobs and their owner reads, compact-self and its send aliases).
4. **role-and-holder** (role, link, role-bearing spawn, answer, claims and their aliases, directory lineage, orchestration, requests).
5. **outputs** (output and the outputs alias: answers and forced wakes).
6. **registration and adoption changes** (descriptor and role fields on re-registration, adopt and reclaim; issuance itself is already secured in 2a).
7. **activity** (report, activity).
8. **attribution** (any-seat writes where impersonation forges only the author).

## Open dependencies and questions for phase 2

- **PRD 003, prime-owned, not decided here (contract f):** q-02 (inbound or outbound limits), q-03 (which prime is always reachable) and q-04 (who is exempt: the operator, the Telegram bridge, system messages).
  - Authentication proceeds without them. Nothing in the bootstrap, the token, session ownership or the gate order depends on their answers.
  - Permission policy assumes no answer.
  - Any q-04 exemption must apply to an authenticated origin. A claimed "system" label, or the machine key alone, never becomes a bypass.
  - If phase 2 finds a choice that does need one of these answers, it stops at that choice and goes to the prime through pm-001.
- **Claude hooks.** Do they run below the Claude process in every launch mode (tmux pane, IDE)? If not, Claude needs a long-lived owner to hold the token.
- **Copilot.** The one-owned-session rule is decided (above). If Copilot later offers a private per-session tool channel, a session-host attachment could replace it for Copilot.
- **The context grant** is folded into the capability (one secret per incarnation, issued only through the secured issuer).
