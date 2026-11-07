# Workshop 001: capability bootstrap (plan 221, PRD 003 req-10, F88)

**Decision wanted:** how each seat incarnation gets a private capability that every seat-authority request must present, so phase 2 can gate the routes in `harness/scripts/route-inventory.json` on it. Security and reviewer verdicts on this file's commit come before phase 2 is planned (bp-0002).

**Recommendation:** option C. A per-incarnation capability token is the per-request control (one hash compare). It is minted only after bootstrap evidence: a single-use spawn nonce for spawned and revived seats, or Unix-socket peer credentials for everything else. Short-lived clients (the shim and hooks) use peer credentials on each call instead of holding the token. Phase 2 gates the route classes in this order: send-as, inbox-claim, controls, role-and-holder, outputs, registration and adoption, activity, attribution.

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

## Threat model and its honest limit

The attacker in F88 is **a local caller holding the daemon key**: in practice another seat, or a script it runs, acting through pij's API as the same OS user. The control raises impersonation from "read the public roster" to "steal another process's private state": its memory, its environment (`ps -E` on macOS, `/proc/<pid>/environ` on Linux), its files, or running code inside its process tree (for example typing into its tmux pane). **A same-user attacker can do all of those**, and no design here stops it; that needs separate OS users per seat, which is out of scope. The capability makes impersonation a deliberate OS-level act, visible in its own right, rather than an API call that looks like normal use. Every option below is judged against that bar.

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
- **Per-request cost: the deciding problem.** A connecting pid that isn't the harness itself needs an ancestry walk. On Linux that is `/proc` reads (no launch). On macOS it is C27's one single-pid `ps` per host-named ancestor. Long-lived clients call heartbeat, inbox and activity per tick per seat, so B alone launches per tick per seat on macOS. AGENTS.md forbids that shape (pij-fleet#62, plan 180).
- **Residual.** A process that runs inside the victim's tree (a typed command in its pane) passes, and so does a pid reused after the victim died, unless process start is checked. F88 also notes peer credentials can't tell apart two sessions sharing one host process. req-10 itself calls peer credentials bootstrap evidence, not the whole control.

### C. Both: peer credentials (or a spawn nonce) as bootstrap evidence, the token as the per-request control

- **Bootstrap**, done once per incarnation at registration and never from roster identifiers alone:
  - **Spawned and revived seats:** the daemon mints a single-use spawn nonce, stores its digest with the spawn, and passes it in the spawn environment next to `PIJ_SPAWN_ID`. The first registration that presents it is consumed. A second claimant, the real seat arriving late, is refused loudly, so a theft is visible. The nonce is worthless after first use, which shrinks the environment exposure from "the seat's lifetime" to "the seconds before it registers".
  - **Hand-started and adopted seats, and any re-registration without the current token** (daemon restart with the digest lost, a client that lost its memory): register over the Unix socket. The peer pid must equal the claimed harness pid with the same process start, or be its descendant (one ancestry read, at registration rate, never per tick).
  - **Re-registration of the same incarnation** must present the current token (it may rotate) or redo peer-credential bootstrap. A TCP registration with only public facts gets no token. This is the fix for F88 row 2.
- **The control, per request.** The registration response carries the token. The daemon keeps only its SHA-256 digest, persisted with the seat so a daemon bounce doesn't strand clients. Long-lived clients (the Copilot native, OMP and Pi extensions) hold it in memory and send it on every seat-authority request, for example as `X-Pij-Seat-Capability`. The daemon resolves the seat from the token, not from the claims. The claims must then agree with it, or the request is refused by name.
- **Short-lived clients** (the `pij` shim, `pij-rs`, the Claude, Codex and Copilot hooks and status lines) connect over the Unix socket. The daemon maps the peer pid to a registered incarnation by ancestry, so no secret ever sits in the environment or a file. Their call rate is agent-action rate (a `pij send`, a turn hook), never a per-tick loop. The ancestry result is cached per live ancestor (pid plus start) with that process's lifetime, so repeated calls from one tool shell are free.
- **Subprocesses** therefore act as their seat through ancestry, never through an inherited secret. A process outside the tree can't, with or without the daemon key.
- **Rotation and death.** A new token per incarnation. Its digest is cleared on tombstone, on supersede and when liveness marks the incarnation dead. Rotation on re-registration is allowed but not required.
- **Per-request cost.** Token requests cost one SHA-256 of 32 bytes, a constant-time compare and a map lookup: no launch. Peer-credential requests cost one `getsockopt` per connection, plus an ancestry walk only on a cache miss: `/proc` on Linux, and on macOS C27's single-pid `ps` per host-named ancestor, at agent-action rate. **Nothing launches per tick per seat.** Phase 2's backpressure must state the measured rate and record any new launch site in `spawn_sites.rs`.
- **Residual for a daemon-key holder without the capability:**
  - **Still allowed:** every machine-public route (by policy: roster, events, cards, reads), ordinary spawning without a role, sidecar, hold and release, and the job-token hook with its own token.
  - **Needs an OS-level act:** impersonating a seat means reading a long-lived client's memory, winning the race for an unused spawn nonce (and being caught when the real seat registers), or running code inside the victim's process tree.
  - **Federation:** a paired machine's key stays scoped to its three routes, with sends attributed to the machine alias (unchanged, outside this design).

## Why C

- **A alone** can't bootstrap hand-started or adopted seats without reissuing from public facts, and to serve the shim and hooks it puts a long-lived secret in every subprocess's environment.
- **B alone** launches `ps` per tick per seat on macOS for the long-lived clients, and can't tell apart two sessions in one host.
- **C** uses each mechanism where it's strong:
  - peer credentials (and the single-use nonce) as rare bootstrap evidence;
  - a hash compare per request for long-lived clients;
  - ancestry for short-lived ones at their natural rate.

  No secret outlives registration in the environment.

## Rollout (old clients during the change)

1. **Phase 2a, additive, nothing refused.**
   - Add the Unix socket listener next to TCP, both serving one router.
   - Mint, persist (digest) and return tokens.
   - Clients send the token and switch to the socket.
   - The daemon counts each seat-authority request as capability, peer-credential, or neither, per route class. Expose this in `/health`, so the rollout is measured, not assumed.
2. **Phase 2b, enforce class by class**, in the order below. Each class flips from count to refuse only after its "neither" count is zero across a working day on the live fleet. Each class has a named refusal (`E-RS-SEAT-CAPABILITY`) that tells an old client what to update.
3. **TCP** stays for machine-public routes and paired machines. Seat-authority requests over TCP must carry the token.

## Route classes in the order phase 2 should gate them

1. **send-as** (`/v1/send`, `/v1/shim/send`): the highest-impact impersonation (sending, FYI and role requests as another seat), and the one #152 round 4 showed still accepts an asserted session id.
2. **inbox-claim** (inbox, acks, heartbeat, FYI claim, dispatch ack): stealing or committing another seat's deliveries.
3. **controls** (close, revive override, inbox release, background jobs, compact-self).
4. **role-and-holder** (role, link, spawn with a role, answer, claims, directory lineage, orchestration, requests).
5. **outputs** (output, outputs: answers and forced wakes).
6. **registration and adoption** (re-registration and adopt require the token or peer-credential bootstrap; this also closes F88 row 2's context-grant reissue).
7. **activity** (report, activity).
8. **attribution** (any-seat writes where impersonation forges only the author).

## Questions phase 2 must answer before code

- Does any harness host two seats in one process? If yes, short-lived calls from that host need session evidence beyond the peer pid.
- Do Claude hooks run as descendants of the Claude process in every launch mode (tmux pane, IDE)? If not, Claude needs a long-lived holder for the token.
- Is the context grant folded into the capability (one secret per incarnation) or kept separate? This workshop suggests folding it in.
