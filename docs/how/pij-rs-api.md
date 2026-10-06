# pij-rs API and governance contracts

This is the stable consumer reference and refusal ledger for the Rust cutover. `pij` serves an explicitly routed rs operation or refuses; it never silently runs the TypeScript daemon. `PIJ_DAEMON_GENERATION=legacy` is not an escape. Missing credentials, an absent daemon, unknown verbs/leaves and unsupported flags do not authorize another store. `pij sessions` reads rs rows only, never a cross-generation union.

## Evidence and wire format

Canonical fixtures, **not new live captures**:

- [`governance-routes.json`](../../crates/testkit/fixtures/golden/api/governance-routes.json): request/response cases, refusal cases, isolation contract and deterministic fixture values.
- [`governance-events.json`](../../crates/testkit/fixtures/golden/api/governance-events.json): Hello, exact frames, decoded payloads and ordering invariants.
- [`error-envelopes.json`](../../crates/testkit/fixtures/golden/api/error-envelopes.json): shared complete v2 error examples, including `not_found` and `cursor_reset`; snake_case is the runtime wire contract.

Historical plan copies remain receipts, not runtime schema dependencies. Fixture timestamps, seat ids, paths and sequences are examples, not evidence of a deployment. PM owns isolated runtime captures and composition validation; production and chainglass acceptance remain prime-owned. The existing plan-root `impl-guide.dd.json` stays where it is; this cutover does not move it.

Executed client captures live separately in [`governance-client-parity.json`](../../crates/testkit/fixtures/golden/api/governance-client-parity.json): 29 mutation and 9 refusal pairs against identical scripted responses, 18 same-daemon CLI pairs, 4 live HTTP/shim pairs, and 6 error-policy pairs. Each record retains original stdout, comparison method, and binary/source hashes. Scripted mutation responses prove the client boundary; separate real workflow and OMP canary receipts prove domain effects. Native-only registration and HTTP-only sessions are declared capability probes, not silently omitted failures.

HTTP responses use the complete `Envelope<T>` v2: `ok`, `command`, `v`, and the original `data`, `error`, `meta`, `details` or other present fields. `error` uses the existing **snake_case** ErrorKind wire values, including `not_found` and `cursor_reset`; do not change the production enum/decoder or reinterpret them as `not-found`/`cursor-reset`. A named refusal lives in `details.code`, not a replacement nested error object. For routed HTTP commands, `pij --json` forwards the original complete v2 success **and refusal** envelope bytes: no data-only unwrap, field renaming, filtering or rewrapping. Human output is a presentation, not a separate schema. Native/shim byte parity must be captured against equivalent state; these fixtures do not claim a new parity run.

Native `pij-rs --json` also preserves the original response buffer after version and payload validation; it never re-fetches the body for printing. Both clients append a final newline only when one is absent. Deliberately CLI-computed native results remain computed: spawn's `dispatched`/`bound`/`pid` projection, send's origin/message-id receipt, the compact-self command alias, and aggregated inbox/acknowledgement-warning output. Those are not unchanged daemon responses.

`commit-trailers` is the deliberate CLI-only exception: `pij commit-trailers` forwards narrowly to `pij-rs commit-trailers`, preserving trailer-only stdout, native stderr (including the derivation-tier note), and exit status. There is no commit-trailers HTTP endpoint or JSON wrapper.

`fleet-report` (plan 162) is the second native pass-through: `pij fleet-report <FOLDER>` forwards to `pij-rs fleet-report`, which folds the project's transcripts in-process through Unisphere, reads seats from the store **read-only** (never created, never migrated) and writes `index.html`, `report.json`, `report.js`, `manifest.json` and `tables/` to one folder. It never contacts the daemon. P1 refuses `--format parquet` (`E-RS-FLEET-FORMAT`) and `--prep-target` (`E-RS-FLEET-PREP-TARGET`) by name; the output folder carries names and paths unless `--anonymise`.

### Exit policy

AC4 asserts **JSON byte parity**, not exit-code parity. Existing client policies remain distinct; scripts must inspect the complete envelope and use the policy of the executable they invoked.

| Result | Native `pij-rs` | Shim `pij` |
|---|---:|---:|
| Successful envelope | 0 | 0 |
| `error: refused` | 2 | 4 |
| `error: not_found` | 3 | 4 |
| `error: auth` | 4 | 4 |
| `error: cursor_reset` | 5 | 4 |
| `error: skew` or `adapter`; uncategorized envelope failure | 1 | 4 |
| Native argument/usage error; shim named argument/unported refusal | 2 (Clap stderr, not necessarily JSON) | 4 (named refusal) |
| `commit-trailers`, `fleet-report` native pass-through | Native command status | Same native command status |

Sources: [`exit_code`](../../crates/cli/src/lib.rs), [shim refusal/output handling](../../.omp/extensions/pij/cli.ts). The native-pass-through signal contract is also preserved; this table does not redefine OS signal termination.

## Store health and daemon logs

`GET /health` probes the shared status store's schema and acquires then rolls back
a write transaction; a responsive HTTP listener alone is not healthy. HTTP 200
has `ok:true`, `data.status:"healthy"` and
`data.store:{status:"healthy",timeout_ms:2000}`. A failed or exhausted probe returns
HTTP 503, `ok:false`, `error:"adapter"`, a diagnostic `meta`,
`data.status:"unhealthy"` and `data.store.status:"unavailable"`.
Pool acquisition and SQLite write-lock waits each have a two-second bound;
the health probe itself has a two-second deadline.

Admitted store transactions run to COMMIT/ROLLBACK independently of request
cancellation. A disconnected client may therefore have committed its operation:
absence of a response is not permission to duplicate it. Pool release/acquisition
flushes pending rollback work and repairs any remaining sqlx transaction depth
before reuse, logging `recovery_count` for each detected connection.

After producers stop, shutdown gives tracked delivery/publication work and both
store pools one shared five-second drain deadline. On expiry it logs one line
with role-labelled checked-out connection counts and continues shutdown; the
roles can share a pool. Normal drains preserve admitted writes, but an expired
drain cannot promise their completion. It never waits forever for a leaked lease.

Every newly emitted daemon stdout/stderr line has a UTC RFC 3339 prefix,
including multiline diagnostics. Existing appended log history is not rewritten.
Orderly shutdown and panic unwind drain partial lines; SIGKILL cannot promise it.

## Machine trust, caller identity and roles

Every `/v1` route requires a bearer key. The key grants **machine access**, not a seat identity or authority over another seat. Callers are within that trusted-machine boundary; `CallerContext` is not a cryptographically authenticated per-seat credential. The daemon resolves the caller using its existing identity service and observed pane/process binding. An asserted `PIJ_SESSION_ID` locates a record; it cannot create a seat or override contradictory pane evidence. Caller-supplied pid/start fields in the shim context are diagnostic, not adoption evidence. Never substitute a short-lived CLI pid for the harness process.

Governance families accept `{ "argv": ["project", "list"], "caller": { "TMUX_PANE": "%10", "cwd": "/work/project" } }`. `argv` includes the family; a mismatched route/family refuses. Native typed role/answer/close/reap requests converge on the same daemon operation as shim argv, not an independent semantic parser. Do not mix typed operation fields with argv. `--actor`, `actor` or `assigned_by`, where accepted, must match the resolved caller: they never grant impersonation.

`RoleService` is authoritative: asserted roles live in `seat_roles`, joined into whoami, `/v1/seats`, StateCard and phonehome. A descriptor's stale role does not win. The projected role is optional (`Option<String>`): a string is an assertion; JSON `null` means no asserted role, never an inferred `worker` or empty string. Consumers must preserve null versus omission and must not reinterpret an omitted role as a request to clear it. Adoption/registration with role omitted preserves an existing assertion; a fresh unstamped seat remains null. Explicit null on adopt/register is not an unset operation. Use `pij role <seat> --unset` or typed `/v1/role` with `"role": null`; omitting role from that typed mutation is invalid.

Self or the target's **current recorded parent** may assert/unset its role; an unrelated seat gets `E-RS-OWNERSHIP`. Protocol may require governors to designate roles, but the service's self-or-parent contract is explicit. An asserted display role, even `prime`, confers no general governance or remote-control authority. Narrow exceptions are [assume-dead revive of a parentless seat](#revive) and [parking a blocked extension inbox head](#extension-inbox-recovery), which accept an authoritative `prime` role. Prime designation is a separate orchestration record.

Sources: [`identity.rs`](../../crates/daemon/src/http/identity.rs), [`role.rs`](../../crates/daemon/src/http/role.rs), [`types.rs`](../../crates/daemon/src/http/types.rs), [`governance.rs`](../../crates/daemon/src/http/governance.rs).

## Spawn harness choice and retirement policy

**OMP and Pi are distinct harnesses, with no shared default.** Native `pij-rs spawn --harness <h>`, HTTP `POST /v1/spawn` and in-process `pij_spawn({harness: …})` require an explicit selection from `omp`, `pi`, `claude`, `copilot` and `codex`; the parent runtime is not a default. `--bin` (HTTP `executable`, in-process `bin`) overrides only the executable path, never selects the harness. Bare selectors such as `--harness pi --bin omp` refuse with the fix `--harness omp`; a known different harness basename also refuses inside an absolute path. A matching absolute path, such as `--harness omp --bin /absolute/path/to/omp`, is accepted. OMP uses provider-qualified model ids such as `github-copilot/gpt-6-astra`; Pi uses its own installed catalog's exact id, such as `gpt-5.6-sol`, with provider qualification when needed. Model availability must be established in the selected runtime, not inferred from the other.

| Harness | Required selection | Native model/effort translation |
|---|---|---|
| OMP | `--harness omp` or `"harness":"omp"` | `--model github-copilot/gpt-6-astra --effort high` becomes OMP `--model github-copilot/gpt-6-astra --thinking high` |
| Pi | `--harness pi` or `"harness":"pi"` | `--model gpt-5.6-sol --effort high` becomes Pi `--model gpt-5.6-sol:high` |

These are argument-shape examples, not catalog or runtime receipts. `pij models` catalog enumeration remains unported (`E-RS-UNPORTED`): agents still cannot enumerate per-harness catalogs through pij, and no native catalog command is supplied. Consult the selected harness's installed catalog, then prove the choice with a canary and actual runtime evidence. Existing native launch flags are otherwise unchanged; native spawn still has no legacy layout/task/branch/plan-id flags.

The daemon's `Config.retired_harnesses` reads comma-separated `PIJ_RETIRED_HARNESSES`. The generic default is empty: no harness is retired. This machine's `just bounce-rs` recipe explicitly sets `PIJ_RETIRED_HARNESSES=pi`; Pi is not hard-coded as globally retired. With that policy, native and in-process Pi spawn refuse with `harness pi is retired on this machine; use omp (or pass --allow-retired)`.

An intentional native `pij-rs spawn --harness pi --allow-retired …` sends HTTP `"allow_retired":true` to `/v1/spawn`. The daemon accepts the override with a spine event; inspect `pij spine events --peer <new-seat> --json` and retain that evidence. Omitted or false `allow_retired` does not bypass policy. The override does not rewrite the policy or change existing seats' registration, receiver ownership or caller authority.

The existing `GET /health` v2 response adds `data.retired_harnesses`, an array of harness names (`[]` for the generic configuration, `["pi"]` for this machine's recipe). In-process spawn reads this through `client.health()` before local pane or spawn-expectation mutation; it does not re-read `PIJ_RETIRED_HARNESSES` in the extension environment or infer policy from the parent harness. OMP and Pi use their distinct local launch paths; external harness choices use native `/v1/spawn`. Use native `--allow-retired` when deliberately overriding retirement, never a `bin` selector. `pij-rs spawn --role pm|worker|pa` (HTTP `role` plus `caller`; in-process `pij_spawn({role})` for claude/copilot/codex) stamps the child's role from above; see [Seat roles](#governance-route-inventory). In-process omp/pi spawn refuses `role` until pij-fleet#25.

## Registration and tombstone continuity

Native registration bounds each `ps` observation to **1 second** and the complete
host/ancestry observation to **5 seconds**. These are fail-closed resource brakes,
not identity policy; process-start sandwiches, executable and ancestry checks
are unchanged. Timeout refusals name the observed PID and return HTTP 409 with
`details.retryable:true`. Copilot extensions retry that existing typed response
with ordinary registration backoff; it is not a `native-session` Hold.

Every successful `/v1/register` returns and stores a live descriptor (`tombstoned_at: None`). An existing retired address is resumed only with the **same harness AND** at least one continuity witness: the same nonempty harness session (`HARNESS_SESSION_ID`, `harness_session` or `harnessSession`), the same nonempty `spawn_id`, or the recorded process incarnation corroborated by the daemon. Missing values and a matching pane alone prove nothing. Without continuity it returns HTTP400, v2 `error:"refused"`, with `seat <id> is retired (<reason>, <at>); a different session may not take it`, without changing the row; a different harness adds `; harness mismatch: row <h1>, claim <h2>` and is never adopted by a lift. Paneless Claude/Codex/Copilot session selection includes retired addresses, preferring live rows, then greatest `proc_start`, then ascending id; historical duplicates are not host owners. Their existing live-owner, session and pull-shape restrictions remain stronger than the continuity ladder. A lift clears both tombstone fields, keeps the address, harness, parent, authoritative role and Telegram bindings, updates the claimed process/pane, and emits exactly one resume event with `prior_reason`, `old_proc`, `new_proc`, `old_pane` and `new_pane`: `seat.native-resumed` for native Copilot, `seat.resumed` otherwise. Native ownership/HOLD checks, the verified spawn-prebind exception and the `revive` verb are unchanged.

## Adopt continuity order (plan 156)

`/v1/adopt <pane> --harness <h>` picks the seat in this order:

1. **Operator reclaim** (`--reclaim <seat>`, below), when it is supplied.
2. **The conversation's seat.** A seat follows its conversation, live or tombstoned, if it has the same harness. A tombstone lifts with one `seat.resumed`, and the pane's previous registration for this same process is superseded. For **Claude** the daemon derives the session on every adopt from the pane process's own `<claude home>/sessions/<pid>.json`, across every Claude home including `~/.claude-alt`. The record counts only when its `pid` is the process tmux reports and its `procStart` matches that process's observed start, so named resumes resolve too. `--harness-session <id>` is only a cross-check: a mismatch, or a flag with no corroborating record, refuses and writes nothing. For other harnesses the flag is believed only when the process owning this connection, observed with `lsof` and a `ps` parent chain, descends from the pane's harness process. If the conversation is live in another process, adopt refuses and names that pane.
3. **The pane's incumbent**, but only while its recorded `(pid, proc_start)` is alive. A pane id alone is neither identity nor liveness: pane ids and pids both reset at boot. `whoami --pane` applies the same rule, and its refusal names the dead seat.
4. Otherwise a freshly minted name.

`--reclaim <seat>` is an operator override for continuity that cannot heal by itself. It is allowed only when the caller, resolved from its own pane or id, is the target's recorded parent or holds the `prime` role, the harness matches, and the target's process no longer runs. It refuses by name otherwise, before any write. The target keeps its parent and drops its recorded conversation, because that is exactly the evidence that went stale. The status-line heal then records the true one. It emits `seat.reclaimed` with `caller`, old/new proc and pane, `old_harness_session`, `prior_reason` and the evidence.

Claude heals itself. On every render the managed status line checks `whoami --pane`. On a miss (that legacy does not own) or a seat recording a different `session_id`, it fires one detached adopt per pane per 15 s and shows `⛓ registering…` until it lands. This adds about 1–2 ms to a healthy render. The SessionStart hook retries in the background for about 30 s when the daemon is not up yet. Both write RFC 3339-stamped lines to `~/.pij-rs/hook.log`. The daemon also installs a pij-managed Copilot status line (`~/.pij-rs/copilot-statusline-pij.sh`, wired through `~/.copilot/settings.json`, with the prior script backed up). It is display-only and shows a dead or missing seat as such.

## Native Copilot registration

For a verified native Copilot `/v1/register` claim, `harness_session` normally identifies the durable seat in this machine's registry before pane/process heuristics. Restarting the same conversation reuses its id and adopts the observed host tuple and pane without waiting for the old process or death sweep; tombstone lifting follows the shared continuity contract above. Only live same-session rows compete: two live addresses return HTTP 409 with v2 `error:"refused"` and `details.retryable:true`. Tombstoned rows are history; selection prefers a live row, then greatest `proc_start`, then ascending id as a deterministic final tie. Queued jobs, parent, authoritative `seat_roles` assertion and Telegram reply binding retain that address. `pij state` reports the new process and no tombstone; `seat.native-resumed` records `old_proc`, `new_proc`, `old_pane`, `new_pane` and `reason:"native-session-resume"`.

Explicit verified spawn intent is the exception: a live, unbound Copilot prebind with the exact spawn id and observed pane retains the spawner's allocated id and parent, even when the child resumes a saved conversation. The saved live address is retired with `seat.native-superseded` (`successor`, `spawn_id`, `reason:"native-spawn-prebind"`). Each address keeps its own queued jobs and bindings; nothing is retargeted. The allocated id binds normally, so the `/v1/spawn` waiter observes the child it requested rather than timing out on a discarded prebind.

Same-session replacement relies on Copilot allowing only one live host per conversation through `inuse.<pid>.lock`. The daemon verifies the new host but does not probe incumbent liveness or enforce that external single-host guarantee. If Copilot violates it, the second claimant replaces the recorded tuple while the first may still hold an inbox long-poll: two receivers could temporarily share one address. This is an explicit dependency, not daemon-proven exclusivity.

Different sessions cannot claim an existing session's id; same-pane new conversations retain the new-id/supersede behavior. The cross-machine guard would refuse if foreign provenance were ever recorded, but the SQLite registry clears `machine` on write: rows are local at rest, and that branch currently has only synthetic defence-in-depth coverage. Native clients refresh/retry typed races without inventing an identity; ordinary ownership refusals remain terminal. `/v1/adopt` preserves the same 409 `retryable:true` discriminator if its registration service returns `Retryable`; ordinary adoption does not currently enter the native owner-observation path.

A verified native claim for a session with no existing seat is held when its pane belongs to a live local Copilot seat of another native session: HTTP 409, v2 `command:"pij register"`, `error:"refused"`, `details:{retryable:true,hold:"native-session"}`. The reason names the owning seat and says “awaiting resumed session”. This is not admission: no transient identity, binding or queued work is written. Explicit foreign-address, spawn-correlation and supersedes refusals remain refusals; an existing saved session still uses the normal resume path. The extension stays alive, retries with backoff initially capped at 5 seconds, and shows no timeline line for the first 10 seconds, then at most one “waiting for this pane's resumed Copilot session” notice. After 10 minutes of continuous hold, it emits exactly once: “[pij native] still waiting for this pane's resumed Copilot session after 10 min; pij delivery is unavailable in this window”. The hold retry has no attempt limit or deadline: fork termination is the terminating bound, and after that escalation retries continue at a slower 60-second cadence without exiting. Copilot may stop the throwaway fork when resuming the real session; otherwise, once the old owner is swept dead or its exact incarnation is observed dead, an ordinary retry can admit the genuinely new session.

## Verified paneless external admission

`pij inbox register --json` is served through Rust authority for external
Claude/Copilot/Codex callers without `TMUX` or `TMUX_PANE`. The accepted address
is `data.id` in the complete v2 envelope, not a top-level id. The bridge validates
`CLAUDE_CODE_SESSION_ID`, or `COPILOT_AGENT_SESSION_ID` plus its matching UUID
session-state directory, or `CODEX_THREAD_ID` plus its matching readable native
rollout. It never allocates a name or writes a legacy registry/channel.

Native `/v1/register` receives an empty id, exact harness/session, folder and
requesting CLI process tuple. The daemon observes the nearest matching live
external harness ancestor, checks process incarnations and conflicting session
arguments, and allocates/reuses the canonical address under the shared registry
lock. The stored and returned `data.proc` belongs to that **host**, never the
short-lived CLI. Unchanged repeat admission returns `binding:"same"` without a
registry/event mutation. Inherited stale ids do not override verified native
identity; absent/conflicting host evidence refuses. Ordinary native registration
and extension-owned Copilot admission retain their separate protections.

From the same verified native host/session, no manual `PIJ_SESSION_ID` export is
needed for the shim's whoami, phonehome, send or inbox paths. `PEER_ID` below is
another peer's actual registration `data.id`; `MSG_ID` is the message being answered:

```bash
pij inbox register --json
pij whoami --json
pij phonehome --json
pij send "$PEER_ID" 'literal text' --json
pij inbox --wait 30000 --json
pij send "$PEER_ID" 'reply text' --in-reply-to "$MSG_ID" --json
```

`inbox --wait` without milliseconds waits indefinitely; a positive finite timeout
returns an empty claims array when no mail arrives. Success remains the complete
v2 envelope with `data:[...]`; claims contain `job_id` and the original `message`,
including reply correlation. The CLI acknowledges only after successful output.
Self-send remains refused. Paneless pull has `pane:null` and
`native_extension_delivery:false`: it is not extension push attestation or a
model canary, and pushed/extension-owned seats must not run a competing wait.
Bare native clients still need the explicit registered identity their API takes.

Sources: [`registration.rs`](../../crates/daemon/src/registration.rs),
[`shim.rs`](../../crates/daemon/src/http/shim.rs),
[`generation-router.ts`](../../.omp/extensions/pij/adapters/generation-router.ts).

## Extension inbox recovery

In OMP and in Pi, body delivery is acknowledged only when that harness emits the matching
`message_start`; injection returning or a turn starting is not consumption proof.
The ordinary route stays unchanged. An unconsumed message idle for
`PIJ_REDELIVER_IDLE_MS` (default `10000`) is resent through `sendUserMessage`,
not the custom-message queue, with a one-line `[pij resend n]` prefix and message-id
correlation. Starting another turn resets the idle window. The local extension
capture `delivery.resend` records `messageId`, `attempt`, and the recovery reason;
it is not a daemon spine event.

OMP recovery preserves the current human draft without delaying delivery.
After the resend's correlated `message_start` ACK handler, the extension snapshots
the editor and restores it on the next tick, after OMP's user-message clear.
Restoration requires an empty editor and no intervening terminal input; human
typing, submission, or clearing wins. Restored text places the cursor at its end
and is never submitted as part of the resend.

A reclaimed unconsumed message refreshes claim tracking, never directly resends.
Consumption remains deduplicated by message id. The boundary recovery below and
idle fallback share a three-resend limit; after the last resend gets its idle
consumption window, `POST /v1/inbox/ack` with
`"delivery_outcome":"undelivered:harness-swallowed"` terminally fails the claim
instead of recording successful delivery. Escape can trigger the boundary recovery.

Extension body claims use `PIJ_RS_EXT_CLAIM_LEASE_SECS` (default `60`).
While holding an unconsumed claim, the extension sends
`POST /v1/inbox/heartbeat` with `{"seat":"<seat>","job_id":123}` every 20s.
The successful v2 envelope has `data:{"job_id":123,"state":"running"}`; renewal
resets `claimed_at` without incrementing `lease_expirations`, acknowledging
consumption, or recording `ReaderRead`. A recipient-owned terminal body returns
`state:"done"` or `"failed"` without mutation; wrong recipients and controls refuse.

At expiry, a live seat persisted as `working` renews rather than spending a lease.
This working-state check is a **one-directional safety interlock (brake)**:
removing it can only expire/park more mail. Three silent, unprotected lease
expirations park the job as `state:"failed"` with
`outcome:"undelivered:lease-exhausted"`, unblocking the serial queue; ordinary busy
turns do not count as extension failure. Parking remains a **policy**: it changes
the delivery outcome. Neither leases nor resends replay remote controls or change
Claude/Copilot delivery.

### Compaction and resend

- `session_before_compact` suspends claims/resends. The first `session_compact`, `turn_start`, `agent_end`, or `message_start` releases the latch and re-polls on the next 1s tick, even without a push callback. Aborted compaction need not emit `session_compact`; a wall-clock ceiling (`PIJ_COMPACTION_LATCH_MAX_MS`, default 120000ms) also releases it and captures `delivery.compaction-repoll` with boundary `ceiling`.
- The first observed `tool_result`, `turn_end`, or `agent_end` grants one prompt resend after `PIJ_BOUNDARY_GRACE_MS` (default 2000ms), even while busy; matching `message_start` cancels it. The default is max(2× measured OMP drain p95, 2s): five healthy busy-seat samples gave p95=139ms. This rule applies to both OMP and Pi.
- Compaction defers that resend until the latch releases; later retries retain the 10s idle fallback and the shared three-resend limit. Both timing overrides accept positive safe-integer milliseconds; invalid values use the default.
- Heartbeat runs every 20s and before resend; only a validated, matching-job `done`/`failed` response drops pending tracking without reinjection. Legacy/unknown data and heartbeat errors retain the idle resend/parking path, not busy-boundary recovery; failed/unknown forced probes cannot bypass the 20s heartbeat throttle. During stream reconnection, inbox transport work waits for the existing exponential reconnect backoff, while local idle resends remain available.
- Test-only `PIJ_TEST_SWALLOW_INJECT=1` drops OMP custom peer injections for isolated-daemon proof; it does not drop prompt resends.

```bash
pij-rs inbox --seat <seat> --peek --json
pij-rs inbox release --seat <seat> --job <job-id> --evidence 'Observed blocked head' --json
```

Peek distinguishes parked rows from the live head and does not acknowledge them.
Release requires the target's recorded parent or an authoritative `prime` role,
nonempty evidence, and the running extension body head; refusal is decodable.
It parks with `undelivered:operator-released`. This is not the existing
typing-grace release operation, and does not terminate any harness process.

Every park publishes `delivery.parked` addressed to the sender, carrying
`messageId`, `jobId`, `recipient`, `outcome`, and `reason`, plus the failure receipt.
The failed row and both sender events commit together before live publication.
A real queue paired with a fake spine refuses terminal recovery with
`E-RS-INBOX-AUTHORITY-SPLIT`, rather than emitting cursors from another store;
ordinary reads and successful acknowledgements remain supported.
An OMP sender or a Pi sender sees a warning naming the undelivered id and recipient. The recipient
clears that message from its pending badge and visibly names the parked id once.
The badge counts only unconsumed messages not recently resent; it is not a count
of successful delivery or model completion.

### Native Copilot receiver lease and manual recovery

Native Copilot renews `/v1/inbox/heartbeat` with
`{seat,native_session,pid,proc_start,observed_at,observed_seq}` (no `job_id`).
Both progress fields are required nonnegative safe integers: `observed_at` is
the monotonic millisecond timestamp of the latest actual event observation;
`observed_seq` counts actual observations within the receiver. Initial zeroes are valid. Empty reads,
timer ticks, registration and claims are **not observation progress**.
The exact registered session/PID/start tuple remains mandatory.

With no outstanding deliveries, unchanged progress renews normally. With pending,
deferred or running deliveries, `observed_at` must advance to renew;
Working and self-reported Hold do not substitute for observation. Frozen heartbeats
may remain live during the grace period, but **never extend the last-progress
lease deadline**. The daemon counts at most one miss per renewal interval
(`lease / 3`), not per request. At `NATIVE_RECEIVER_STALE_RENEWALS` (K=3) misses,
or the exhausted lease horizon, it returns HTTP 200 with
`data:{state:"stale",reason:"native-receiver-stale",lease_ms:60000,renew_after_ms:20000}`
and no renewal. Thus frozen work expires within three default 20-second
opportunities, not three opportunities plus another lease.

The accepted live shape remains
`data:{state:"live",lease_ms:60000,renew_after_ms:20000}`; while frozen before
the threshold it reports the remaining lease and a shorter renewal interval
when necessary (`0 < renew_after_ms < lease_ms`).
`PIJ_RS_EXT_CLAIM_LEASE_SECS` changes the duration. Register/claim starts the
first lease for an incarnation, but repeated attestation cannot renew it or
erase a stale latch. Actual strictly advancing observation can restore renewal;
parking or an empty queue cannot clear an already-stale latch.
The local sequence may restart only with a strictly newer observation timestamp;
resetting counters or changing sequence alone cannot revive frozen receiving.
If no receiver reconnects after a daemon restart, the expired boot grace is shown
as `native-extension-unavailable`, not invented frozen-observation evidence.
A replacement child first attests its identity, then observes a bounded SDK history
window before sending its initial progress heartbeat or claiming inbox work.
This allows healthy same-host replacement without granting renewal to a timer alone.

SDK probing is workload-gated: the existing consumption/completion observation loop
reads and checks the independent tail only while a native delivery is pending or its
completion is outstanding. There is no permanent one-second idle SDK timer. An empty
receiver still renews its daemon lease and accepts callbacks, but does not poll the SDK;
the next delivery captures a fresh tail in `newCompletion`. Startup observation remains
a single bounded read. Registration success resets its hold cadence before that read;
a startup observation failure is `receive-held`, never a registration retry episode.
Startup hold is immediately visible in extension stderr; `pij state` exposes the
receiver reason only after the unrenewed lease expires, up to 60 seconds later.

The extension treats `stale` as `receive-held`, not a malformed response or an
infinite retry. `pij state` reports `native_receiver_reason:"native-receiver-stale"`
separately from host liveness; human output names the same reason. `pij inbox`
and `--peek` include it in `details.native_receiver_reason` and human-readable
`meta`, including a manual live-lease refusal and subsequent manual recovery.
Inspect extension `receive-held`/`receiver-rebaselined` diagnostics; wait for the
reported actual lease expiry before pulling. A live host or an `idle · active`
card alone does not prove that its receiver is observing.

Expiry still parks native bodies with `undelivered:native-receiver-unavailable`,
publishes sender-addressed `delivery.parked`, and refuses subsequent sends.
`native-receiver-stale` is a diagnostic reason, **not a new parked outcome**;
the enum remains `DeliveryFailure::NativeReceiverUnavailable`.

From the same Copilot pane/session, `pij inbox --json` uses the normal identity
ladder and registry host tuple. While the receiver lease is live it refuses
without claiming or parking anything: HTTP 409, `ok:false`, `error:"refused"`,
`details:{code:"native-receiver-lease-live",retryable:true,expires_in_ms:<remaining>}`.
The reason names the lease and remaining milliseconds. After the extension dies,
already-claimed mail remains unreachable for **up to the remaining lease**
(60 seconds by default); retry after the reported interval. No `--take-over`
override exists: bypassing the wait could race an accepted but unobserved message.

An expired or absent receiver lease admits explicit recovery regardless of
self-reported status. Pending/running native bodies are parked first; only
receiver-unavailable parked bodies are restored, preserving job/message identity
and parking history. Manual pull/ACK never renews receiver presence. A wrong
pane/session still refuses; paneless readers retain their exact host tuple checks.

## Seat roster rows and absence

`GET /v1/seats` returns the v2 `pij seats` envelope with `data: {seats: [...], unavailable: [...]}`, not a bare row array. Each unavailable peer is `{machine, reason}`; retained rows from that peer are stale last-known data, so an empty `unavailable` list and an empty roster are different facts. Active results exclude tombstoned rows. Local rows are role-joined, stamped with the serving machine alias, and projected with `generation: "rs"`.

| Row fields | Wire contract |
|---|---|
| `id`, `harness`, `folder`, `state` | Stable seat id, harness enum, absolute working folder and stored mechanical state. OMP, Pi and Copilot seats publish `working`/`idle` at their own turn boundaries through `/v1/activity`, and Claude seats publish them through their managed `UserPromptSubmit` and `Stop` hooks. A seat that has not published since the daemon learned it reads `idle` by default, which is not observed inactivity. |
| `generation`, `relay`, `native_extension_delivery` | Present `"rs"` and boolean flags. False is an explicit value, not omission. Display role does not grant native delivery attestation. |
| `session`, `pane`, `proc`, `semantic_state`, `role`, `parent` | Present even when null. `session` is the serialized native harness-session id (not `harness_session`); null is unknown. `pane:null` is paneless; `proc:null` is no bound process identity. Non-null `proc` is `{pid, proc_start}`; never interpret pid alone as identity. Null semantic state/role means no declaration/assertion; null parent means no recorded governing seat. |
| `extension_build`, `extension_path` | Present string or null on current-daemon whoami, seats and state responses. Build is the loaded runtime extension's `<sha10>`, `<sha10>+dirty`, or `hash:<12 hex>`; path is its resolved real source directory, not the seat's working folder. Null means no build was reported (a pre-144 extension in OMP or Pi, or a harness without this feature), never a retroactive guess from today's checkout. Older daemons omit these keys entirely. |
| `badge`, `last_event_at` | Current-daemon rows carry the worst-first row-fact badge and the highest-sequence seat event's epoch-ms timestamp, or null when no event exists. Both come from the roster's SQL snapshot, without per-seat queries. Older remote daemons may omit badge; absent remote freshness is null, not proof of no remote events. |
| `machine` | Optional in the base descriptor serialization; this HTTP handler stamps it for local rows. Preserve the machine alias in federated views rather than guessing from seat ids. |
| `spawn_id`, `model`, `provider`, `effort` | Omitted when unknown/unset (`skip_serializing_if`), not emitted as null; absence is not an inferred launch, model or effort. |
| `cross_session_inbound_accept` | Omitted means unknown, treated as closed; explicit false differs from unknown and true. |
| `tombstoned_at`, `tombstone_reason` | Omitted when absent in descriptor/history serialization. Tombstones are excluded from this active-roster endpoint, not proof that their history never existed. |

These are flattened descriptor fields plus HTTP projection metadata, **not StateCard**: the latter has its own camelCase projection and explicit `unsupported` list. Preserve null versus omission when comparing envelopes. RoleService owns the role join.

OMP and Pi each capture build identity once at their runtime's extension load and reuse it on registration, adoption and re-registration. Git identity uses the real extension directory's repository HEAD, with `+dirty` scoped to that directory; a standalone install hashes sorted relative paths and bytes from `index.ts`, `adapters/*.ts` and `core/*.ts`. Later disk changes do not relabel the running extension. A registration without metadata clears any prior values. This is runtime-reported provenance, not a daemon-verified attestation. Human `pij whoami` prints `extension: <build> (<path>)`; OMP's footer and Pi's footer each keep their loaded build and dirty marker visible. Build identity remains JSON-only for list: the native human view has no width-budgeted build column, and the shim's existing list route is JSON-only.

Direct HTTP `/v1/seats` accepts exact `harness`, `folder`, `parent` filters, `scope=local`, and `here=<absolute-path>`; another scope value or relative `here` refuses. Default scope may include federated last-known rows. Shim `pij list` forwards `--harness`, `--folder`, `--parent` and `--scope local` as URL-encoded GET parameters, preserving literal values. Shim and native `pij-rs list --here` select rows whose recorded `folder` equals the caller's canonical cwd; the flag is boolean (`--here`, `--here=true`, `--here=false`), never a path value. Existing absolute paths are canonicalised; unavailable absolute paths retain their literal spelling, never the daemon cwd. POST accepts `{argv:["list","--here"],caller:{cwd:"/absolute/path"}}`, sharing anomalies' boolean/path helpers. GET and POST query `here=<absolute-path>` use the same projection; other filters intersect rather than widen it. Native list accepts no other filters, and shim `pij sessions` has no query flags. Legacy `--prime`, `--role`, `--archived` and tree filters remain named refusals. `/v1/shim/sessions` is a distinct rs projection, never a union with the legacy store.

```bash
pij list --harness omp --folder '/work/project with spaces' --parent <seat> --scope local --json
pij-rs list --here --json # caller cwd, not daemon cwd
pij sessions --json      # rs session projection: no query flags
```

| Route | Scope | Refusal |
|---|---|---|
| GET/POST `/v1/seats?here=<absolute-path>`; POST argv `list [--here]` | Canonical caller folder equality; preserves `unavailable` and existing exact filters | Relative query/caller cwd or path-valued argv `--here` returns a v2 `E-RS-ARG` envelope |

The additive `list_here_capture` in [`governance-client-parity.json`](../../crates/testkit/fixtures/golden/api/governance-client-parity.json) retains four actual native/shim byte-equal pairs (bare, true, false, unfiltered) and four argument refusals against one private real-adapter daemon. Its binary hash and captured cwd identify the exercised artifact; this is not production deployment evidence.

That retained capture **predates plan 147**: its roster rows have neither `badge`
nor `last_event_at`. It proves the historical folder-filter/argument and
native/shim byte-parity contract, not the current status-row shape. The private
capture daemon and scratch state were removed; the current isolated status
row/card evidence comes from plan 147's isolated capture, not a recapture of those old CLI pairs.

## State card and badge vocabulary

`POST /v1/state` with `{"id":"<seat>"}` returns the same `badge` and
`last_event_at` as the roster for the same stored facts. These two names are
literal, including the underscore in `last_event_at`; the existing card's other
camelCase fields are unchanged. Freshness follows event **sequence**, not the
greatest timestamp: a later committed event can have an earlier supplied clock.

Status reports (`hold`, `waiting`, `blocked`, and the other semantic words) are
first-person declarations, **never delivery or inbox-claim gates**, for every
harness. No separate operator mailbox-hold field exists. Per-job typing holds,
actual recipient consent, native incarnation/context checks and receiver leases
remain independent safety mechanisms. Native typing snapshots retain deprecated
`semantic_hold`, **always `false` since plan 155**, so a daemon bounce remains
compatible with already-running old receivers. New receivers ignore the field
entirely, whether absent or present; preflight validates incarnation, not status.
Remove it only after every live native receiver is positively verified at or
past the field-ignoring plan-155 build (initial `store.mjs` SHA-256
`16a6020da089021cf0d6d0965e35974c0919e1f3e734ced1a98da48a61288fc5`).
Unknown loaded builds block removal; current checkout bytes or a live host do
not prove what an old child loaded. Removal is a tracked follow-up
is outside this plan; once that condition holds, deletion is daemon-only.

`data.deliveryDeferrals` is the current live-job projection:
`[{job_id,msg_id,reason,count,since_ms}]`. Every daemon delivery/drain deferral
increments the durable count; `since_ms` is the first deferred attempt's epoch
milliseconds, including across later reason changes. Both state CLIs show
`delivery deferred: <reason> ×N since <epoch-ms> ms`.
Success or terminal disposition removes the active projection, not the job's
historical diagnostics. Explicit extension `/v1/hold` scheduling remains in
`held[]`; it is not counted as a daemon drain attempt.

The job fact and any sampled `delivery.held` event commit atomically. First
deferral emits immediately, then at most one event per 60 seconds per job,
regardless of reason changes and surviving daemon restart. Events add `job_id`,
`deferral_count` and `reason_changes` (transitions since the previous event, zero
on the first) alongside the current `reason` and `since_ms`; counts are not ACKs.
Deferrals include composer safety, missing binding/pane/transport, pending
consent, and transport errors. Not counted here: native Copilot inbox-pull holds
(the reason is returned in the pull response and `native_receiver_reason` in
`pij state`) and held-for-approval socket backoff, which stops at its attempt limit. Diagnostic publication failure cannot bypass the
original retry/backoff or consent policy.
Like other atomic queue/spine mutations, counted deferral publication refuses a
real Queue paired with a fake Spine (`E-RS-INBOX-AUTHORITY-SPLIT`), rather than
broadcasting a cursor allocated by another store. The body and retry policy remain.

`data.sessionStatus` holds the seat's session facts, read in-process from the harness
transcript through `SessionStatusPort` (plan 157, adapter `pij-unisphere` over
`unisphere-sdk`). Its `outcome` is one of:
- `known`: carries `status`, `cacheState` and `elapsedMs`.
- `unbound`: no `harness_session`, so the source isn't asked.
- `unsupported` (`harness`): Claude (`~/.claude*/projects`), OMP
  (`~/.omp/agent/sessions`), Codex (`$CODEX_HOME/sessions`, default
  `~/.codex/sessions`) and Copilot CLI (`~/.copilot/session-state/<id>/events.jsonl`)
  are readable; Pi is not yet. **Copilot context gap:** Copilot persists no
  per-call input or cache usage, so a Copilot seat's `contextUsedTokens` is always
  unknown. The cold-wake guard therefore allows every send to a Copilot seat
  (`unknown: context size unknown`).
- `not-found` (`detail`)
- `failed` (`error`): the rest of the card is still served.

Each fact is `{"value":V,"basis":"native|derived|table@vN|mtime-fallback"}` or
`{"basis":"unknown"}`. There is never a null, and never a 0 standing in for unknown.
`status` covers:
- `model`
- `contextUsedTokens` and `contextWindowTokens`
- `lastCallAtMs`
- the last call's input, cache-read, 5-minute and 1-hour cache-write tokens
- `cacheTtl` (`five-minutes|one-hour`)
- `compactions`
- `reset`: why the source re-read from the start. It is absent on the warm path.

`cacheState` is derived on the daemon clock as `warm` (`expiresInMs`) or `cold`
(`expiredForMs`). It is unknown when the last call time or the TTL is unknown.
The source keeps one read position per seat, so a warm read folds only appended bytes.
Busy/idle is `state`, never this block. The human CLI prints one `session:` line.

**Size and coldness (plan 160).** Next to `sessionStatus`, the card carries fields derived on the daemon clock. Each is absent when its fact is unknown:
- `contextUsed`: context tokens in use.
- `idleMs`: milliseconds since the last API call.
- `cacheState`: `{state: warm|cold, expiresInMs|expiredForMs}`.
- `coldWake`: `{wouldRefuse, estimateUsd?}`. This is `cold_wake::check` itself, so it is exactly what the guard would do with a normal send now.

`sizeLines` holds the rendered human lines, which every client prints verbatim:
- `context 720k / 1M · last call 2h ago · cache 1h (cold 1h) · 3 compactions`;
- when the guard would refuse, `❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi holds it for $0 now)`.

`GET`/`POST /v1/seats?sizes=true` adds the same `sessionStatus` and derived fields to each **local** row, plus `sizeColumns` (`[CTX, IDLE, CACHE, ❄-or-empty]`). It is opt-in, so federation fan-in and extension rosters don't pay for it. The local seats are read in parallel with a 200 ms wait each. A seat that does not answer in time shows `?`, and its read keeps going in the background, so the next list is warm. Both `pij list` (shim) and `pij-rs list` ask for sizes. Their human output is one table (golden [`cli/list-sized.txt`](../../crates/testkit/fixtures/golden/cli/list-sized.txt) from [`cli/list-sized.json`](../../crates/testkit/fixtures/golden/cli/list-sized.json)):
```
  SEAT             HARNESS  STATE    CTX   IDLE  CACHE
❄ pij-cold-claude  claude   idle     720k  2h    cold 1h
  pij-warm-omp     omp      working  57k   3m    warm
```

The one closed severity order is the exported
[`pij_core::status::BADGE_SEVERITY`](../../crates/core/src/status.rs).
[`badge_of`](../../crates/core/src/status.rs) selects the worst mechanical state
and latest declaration for **each open assignment**, plus an unscoped declaration
when present. Closing an assignment removes its declaration from the badge even
if the legacy single `semantic_state` still contains it. No inputs means
`unknown`. Consumers may colour by these strings; they must not derive a second
badge or severity order.

`semantic_state` on a roster row (`semanticState` on its card) is the legacy
single descriptor declaration; `badge` is the worst current declaration across
open assignments and the mechanical axis. They answer different questions and
may disagree in **both** directions: unscoped `report clear` clears only the
descriptor, leaving `semanticState: null` beside `badge: "blocked"` when an open
assignment still declares blocked; closing that assignment can leave the stale
descriptor `semanticState: "blocked"` beside `badge: "idle"`.
Use `report clear --assignment <id>` to clear a task's declaration without
closing its assignment. Assignment lookup and ownership use the same resolution
as `report state --assignment`, refusing with `E-RS-ASSIGNMENT-UNKNOWN` or
`E-RS-ASSIGNMENT-NOT-YOURS`. Unscoped clear retains its descriptor-only meaning.

The semantic word list is [`SemanticState::WORDS`](../../crates/core/src/model.rs),
generated from the same definitions as the enum, parser and `SemanticState::ALL`;
both CLI help and report refusals read it.

| Priority | Badge | Meaning and current reachability |
|---|---|---|
| 1 | `dead` | Reserved mechanical failure. **Not reported by the 147 badge:** it never consults process liveness or tombstones. Read the card's separate `liveness`. |
| 2 | `failed` | Declared failure; reachable through `pij report state failed`. |
| 3 | `stalled` | Reserved claims-working plus silent event history. **Not reachable yet:** needs native working publication and its age projection (148). |
| 4 | `blocked` | Declared external blocker; reachable. |
| 5 | `question` | Declared human question; reachable. |
| 6 | `hold` | Declared intentional hold; reachable. |
| 7 | `stopped` | Reserved process suspension. **Not reachable:** no suspension observation is published. |
| 8 | `unknown` | Pure-function fallback when neither axis has inputs. Current registered rows have a stored mechanical state, so this is not currently produced by their normal badge path. |
| 9 | `waiting` | Declared wait for external work; reachable. |
| 10 | `starting` | Reserved pre-bind lifecycle. **Not reachable:** no lifecycle publication path (148). |
| 11 | `working` | Reserved native runtime work observation. **Not reachable:** no production work-state publisher (148). Event recency must never be called working. |
| 12 | `ready` | Declared availability; reachable. |
| 13 | `cancelled` | Declared cancellation; reachable through `pij report state cancelled`. |
| 14 | `done` | Declared completion; reachable while its assignment remains open, or as an unscoped declaration. |
| 15 | `idle` | Current stored mechanical default for **Pi, OMP, Claude, Copilot and Codex**. It means work was **not observed**, not that no work is happening. |

`failed` and `cancelled` also work with `report now --state`; old state records
remain readable. Both are non-nudgeable, like other terminal/parked declarations.

**Liveness remains separate.** The card still performs its existing exact
process-identity probe and reports `active`, `dead`, `recycled` or `unbound`.
It does not change the badge, so focusing a row cannot produce a different
badge merely because the card probes a PID. The roster has **no `liveness`
field** until a batch observation source exists (148). A stored PID is not
evidence of a live process; emitting all-unknown values would incorrectly enable
the consumer's presence-gated idle filter. Tombstoned seats remain excluded from
the roster; their retained card does not acquire a `dead` badge by inference.


Sources: [`types.rs::FederatedRoster / UnavailablePeer`](../../crates/daemon/src/http/types.rs), [`model.rs::SeatDescriptor`](../../crates/core/src/model.rs), [`http/mod.rs::SeatProjection / seats`](../../crates/daemon/src/http/mod.rs).

## FYI held delivery (plan 158)

An FYI is a message the recipient's next action doesn't depend on ([the FYI rule](../../skills/pij/references/routes/peer.md#converse)). It never opens a turn: the daemon holds it until the recipient's next real turn, and it arrives with that turn. The only definition of the appended block is the golden fixture [`fyi/block.txt`](../../crates/testkit/fixtures/golden/fyi/block.txt); clients pass `block` through byte-for-byte and never re-render it.

```bash
pij send <seat> --fyi 'text'           # prints: pij send: held (fyi) — <msg_id>
pij-rs send --to <seat> --fyi 'text'   # native form
```

| Surface | Contract |
|---|---|
| POST `/v1/send` with `"fyi": true` | An ordinary send body plus `fyi`. The daemon stores the FYI durably and opens no turn: no tmux, UDS or extension push. `data` is a Receipt whose `outcome` is `{"outcome":"held","reason":"fyi"}`. Refused (`ok:false`, `error:"refused"`) with a control `command` or a remote `seat@machine` recipient; a tombstoned or unknown recipient gets the same errors as a normal send. |
| Receipt `data.warning` (plan 159) | Present only on a held FYI whose body contains `?`: `this looks like a question; if you need an answer, resend without --fyi`. The FYI is still held; nothing is converted or refused. The shim prints it on the line after `pij send: held (fyi) — <msg_id>`; the OMP/Pi `pij_send` tool appends it on its own line; the Copilot `pij_send` tool returns it as a top-level `warning` beside `receipt`. |
| Warm flush (plan 159) | When the hold that brings a seat's pending FYIs to `FLUSH_AT` (5) finds the seat's prompt cache known **warm**, the daemon delivers them now as one queued message from `pij-bg`, whose whole body is the block beginning `5 FYIs were queued for you:`. Warm needs positive evidence: the last API call is known and within the seat's real cache lifetime, `min(cache TTL, 60 min)`. A `working` state is not evidence, and an unknown TTL, an unknown last call, or no answer within 3 s means hold. The FYIs are claimed in the carrier's own transaction, exactly once (`fyi.delivered` names `message:fyi-flush-<seat>-<ms>`). If a hook or ride-along claimed them first, nothing is queued. A turn boundary (`POST /v1/activity`) never flushes: a seat starting a turn gets the pile with that turn. A failed flush leaves the FYIs pending and never fails the hold. |
| Digest (plan 159) | A block for more than `DIGEST_ABOVE` (5) FYIs, whether ride-along, hook claim or flush, is a digest. It has a header with the total and a count per sender, the newest `DIGEST_NEWEST` (3) in full, numbered by their place in the pile, and a last line naming the read command: `All N were delivered; read them in full with: pij-rs fyi-read --seat <seat> --claimed-at <ms>`. Every FYI is marked delivered. The golden fixture is [`fyi/digest.txt`](../../crates/testkit/fixtures/golden/fyi/digest.txt). |
| POST `/v1/fyi/read` · `pij-rs fyi-read --seat <S> --claimed-at <ms>` | Read-only. Request `{seat, claimed_at_ms}`. `data` is `{seat, count, block}`, where `block` is every FYI that claim delivered, in full, oldest first, headed `N FYIs were delivered to you:` (empty when none match). The CLI prints the block. |
| Ride-along | The next non-control, non-FYI message to the seat carries every pending FYI, appended to the same body as `<original body>\n\n<block>` and never sent as a separate message. The FYIs are claimed by the same transaction that creates the message's durable queue row (`fyi.delivered` names `message:<msg_id>`), so an FYI is never claimed without its carrier. A retry or duplicate of a message the queue or ledger already has claims nothing. A pane-bound direct send (socket or typed) that would carry FYIs is queued instead, with reason `fyi-ride-along`, and the drain worker delivers it moments later. A paneless socket seat's direct sends carry none, so its FYIs arrive through the typed-turn hook. A failed FYI claim never fails the send: the send goes out plain and the FYIs stay pending. No client work is needed. |
| POST `/v1/fyi/claim` | Typed-turn hooks only. Request `{seat?, pane?, native_session?, via}`, where `via` is `hook:claude`, `hook:copilot`, `hook:omp` or `hook:pi`. At least one of `pane` or `native_session` is binding evidence; `seat` may be omitted when `pane` resolves a live seat, and a `seat` that mismatches the pane or session refuses. `data` is `{seat, count, block, ids}`; `count: 0` means `block: ""`. The claim is atomic: pending → delivered exactly once, with one `fyi.delivered` event carrying `via`. Racing claims, or a claim racing a ride-along, never deliver the same FYI twice. |
| `pij-rs fyi-claim --pane <P> [--native-session <S>] --via hook:claude` | Prints the raw `block` to stdout, or nothing at count 0, and exits 0. Global `--json` prints the envelope instead. An unreachable daemon exits non-zero; the calling hook then prints nothing and exits 0, so it never blocks the human's prompt. |
| `POST /v1/state` → `data.pendingFyis` | Pending count on the camelCase card (`pij state <seat> --json`). |
| `/v1/whoami` → `data.pending_fyis` | The same count on whoami (`pij-rs --json whoami --pane "$TMUX_PANE"`). Status lines and footers read it here, with no extra call, and show `✉N` only while N > 0. |
| `/v1/events` kinds `fyi.held`, `fyi.delivered` | `seat` is the recipient. Extensions refresh their count on any `fyi.*` event, or `seat.tombstone`, for their seat. |
| `seat.tombstone` payload `pending_fyis_dropped` | A tombstone drops the seat's pending FYIs and records how many in this field, present only when at least one was dropped. |
| Hook claim window | A hook claim that commits in the daemon and then times out in the client (the hooks give up after 3 s) delivers nothing: those FYIs are claimed at most once, never twice. This is the one remaining loss window. |

A claimed `block` is placed as: Claude `UserPromptSubmit` hook `additionalContext`; OMP and Pi `before_agent_start` custom message `pij-fyi`; Copilot native extension `onUserPromptSubmitted` `additionalContext`.

Source: [`fyi.rs`](../../crates/core/src/fyi.rs).

### Busy/idle publication: POST `/v1/activity`

The request is `{seat, pane?, native_session?, state: "working"|"idle"}`. It takes the same binding evidence as `/v1/fyi/claim`, and a tombstoned seat is refused.
It changes only the seat's mechanical `state` (a targeted write that never rewrites or revives the row), so `pij state` shows `working` during a turn and `idle` after it. Each change publishes a `seat.activity` event. `data` is `{seat, state, changed}`.
- `working` never outlives its process: a tombstone resets it to `idle`, and so does a new incarnation registering the seat.

- The OMP/Pi extension publishes `working` at `turn_start`, and `idle` at `turn_end`, at `agent_end` and on shutdown.
- The Copilot native extension publishes on `assistant.turn_start`, and on `assistant.turn_end`, `session.idle` or shutdown.
- Publications are serialized per session, and a failure never breaks a turn. A route-absent 404 from an older daemon stops publishing.
- Claude seats publish through their managed hooks (plan 157): `UserPromptSubmit` sends `working`, and `Stop` and `StopFailure` send `idle`. `StopFailure` covers a turn that ends on an API error, which does not fire `Stop`. The seat is resolved by pane, via `pij-rs activity`, with the same 3 s bound and silent failure as the FYI claim. Only a prompt submitted to Claude fires `UserPromptSubmit`, so a turn opened another way stays `idle` until the next typed or pasted prompt.
- The cases these hooks cannot see leave a Claude seat reading `working`:
  - **An Esc interrupt** fires no Claude hook: neither `Stop`, `StopFailure`, nor the `idle_prompt` notification (verified live). The seat reads `working` until its next prompt or completed turn. The cold-wake guard repairs the costly case. Three conditions must all hold: the seat is cold by its facts alone, its `working` fact is itself older than the idle threshold, and one probe of its live pane (1 s bound, off the request thread) shows **positive** idle evidence, `HarnessPort::idle`. For Claude, idle evidence means a ready footer, the composer row between its rules, and no spinner row. Then the send is refused, and the seat is corrected to `idle` with `seat.activity` reason `stale working (esc)`. Everything else allows and leaves `working` alone: a younger `working`, no pane, a failed or slow probe, or any other frame (empty, blank, truncated, a permission dialog, a tool run). The `working`-age condition exists because a Claude streaming a reply shows no spinner and looks idle (live 2.1.284 frames).
  - **A killed or exited process** fires nothing. The death sweep's tombstone, or a new incarnation registering the seat, resets it to `idle`.
  - While a seat reads `working`, the cold-wake guard treats it as busy and allows the send. That is the safe direction for a brake.

### Cold-wake guard (plan 157 phase 2)

Waking a seat whose prompt cache has expired re-writes its whole context at cache-write prices. The daemon therefore refuses a waking send to a **cold** recipient. The constants, the price table and the pure rule live in [`cold_wake.rs`](../../crates/core/src/cold_wake.rs): context over `COLD_CONTEXT_TOKENS`, last API call older than `COLD_IDLE_MS`, and the daemon does not see the seat working. The guard is a brake: it only ever refuses, and it never changes what is delivered.

```bash
pij send <seat> --force --reason '<why>' 'text'             # shim → /v1/shim/send argv verbatim
pij-rs send --to <seat> --force --reason '<why>' --body 'text'   # native form
```

| Surface | Contract |
|---|---|
| Refusal | HTTP 400, `{ok:false, error:"refused", meta:…}`. Nothing is sent. Since plan 160 the meta lists the sender's options, each priced (golden [`cold-wake/refusal.txt`](../../crates/testkit/fixtures/golden/cold-wake/refusal.txt)):<br>`E-RS-COLD-WAKE: ❄ pij-x is cold (720k, idle 1h52m). Options:`<br>`  --fyi                  hold for its next turn          $0 now`<br>`  --force --reason "…"   wake on its current model       ~$6.45`<br>`Estimates at list price: a 1-hour cache write of the whole context, plus about 5 calls.`<br>The wake estimate is `cold_wake::wake_estimate_usd`. The first call writes the whole context to the 1-hour cache. Each of the 4 further calls reads the context so far, writes ~1,585 new tokens and outputs ~590, which are the 2026-09-28 usage study's means. Every call outputs ~590. Prices come from the `PRICES_USD_PER_MTOK` table (Opus 5.5, Sonnet 5.5, Haiku 4.5, list prices checked 2026-10-01). Model ids are normalised first, and an unpriced model says `price unknown`. The shim prints the meta alone on stderr; the OMP/Pi and Copilot `pij_send` tools return it verbatim as the tool error. |
| POST `/v1/send` `"force": true, "reason": "<why>"` | Wakes a cold recipient anyway. `force` without a non-empty `reason` is refused with a meta starting `E-RS-COLD-WAKE:` that names `--reason`. The tools refuse it too, before any request. |
| Spine `send.cold-wake-forced` | The audit of every forced cold wake, persisted before the wake. `seat` is the recipient; payload `{from, msg_id, reason, context_tokens, idle_ms, model, estimate_usd}`. |
| Receipt `data.cold_check` | `clear`, `busy`, `forced` or `unknown: <why>` on every guarded send; absent when the guard did not run. The shim prints `— cold-check: <verdict>`; the OMP/Pi tool appends `(cold-check: <verdict>)`. |
| Unknown ⇒ allow | Any fact the rule needs but cannot establish (unbound or unsupported harness, no transcript, a source error, unknown context or last-call time) allows the send as `unknown: <why>`. The guard waits 3 s for the recipient's session facts; after that the send is allowed as `unknown: no answer within 3s`. |
| Never guarded | FYIs (`fyi:true` opens no turn), controls (`command`), and forwarded (federated) sends: a `seat@machine` send is not checked on either daemon, because only the originating sender could choose `--fyi` or `--force`. |
| One recipient per send | rs has no multi-recipient send, and the shim refuses broadcasts, so the per-recipient rule is met by one recipient per send. |
| Remote peers | A message forwarded from another machine (`from_machine` set) is delivered unguarded, so a remote peer can still cold-wake a local seat. The receiving daemon cannot offer that sender `--fyi` or `--force`. |
| `pij bounce` announcement | The bounce announcement is an ordinary send from `pij-daemon`. A cold seat refuses it, so the bounce output lists `announcement failed … E-RS-COLD-WAKE` for each cold seat, and those seats are not woken. |
| Cursor warm-up | At daemon start every live, bound seat's session is read once in the background, one at a time with a pause between seats. The first send after a bounce is then a warm read. A read the guard stops waiting for still completes and keeps its cursor. |

Sources: [`cold_wake.rs`](../../crates/core/src/cold_wake.rs) (rule), [`http/cold_wake.rs`](../../crates/daemon/src/http/cold_wake.rs) (guard and audit).

## Governance route inventory

All POST families below use argv/caller unless the typed alternative is named. Unknown leaves refuse; unsupported flags are not dropped. These are the daemon routes, not invented `/v1/report/<leaf>` endpoints.

| Method and path | CLI / operation | Canonical case ids |
|---|---|---|
| POST `/v1/project` | `project create <description> [--repo <path>] [--plan <path>] [--prime <id>]`; `list`; `show <slug>`; `set <slug> [--description <text>] [--repo <path>] [--plan <path>] [--prime <id>]` | `project-create`, `project-list`, `project-show`, `project-set` |
| POST `/v1/stream` | `stream create --project <slug> --slug <stream> [--base <ref>] [--ordinal N] [--root <path>]`; `list [--project <slug>]`; `show <allocation>`; `close <allocation>` | `stream-create`, `stream-list`, `stream-show`, `stream-close` |
| POST `/v1/fence` | `fence set <stream> --paths <a,b> [--shared <x,y>]`; `show [--stream <stream>] [--path <path>]` | `fence-set`, `fence-show` |
| POST `/v1/dispatch` | `dispatch <seat> --packet <path> [--wait[=MS]]` | `dispatch-create` |
| POST `/v1/ack` | `ack <dispatch> --packet-sha <sha256>` | `dispatch-ack` |
| POST `/v1/canary` | `canary <seat> [--expect-model <model>] [--wait[=MS]]` | `canary-verified` |
| POST `/v1/attest` | `attest <seat> --plan-id <id>` | `attest-plan` |
| POST `/v1/task` | `task set <seat> <task> [--project <slug>]`; `close <assignment> --reason <done/cancelled/failed/superseded>` | `task-set`, `task-close` |
| POST `/v1/node` | `node show <seat>` | `node-show` |
| POST `/v1/orchestration` | `orchestration baton define/list/show/request/grant/return/reclaim`; `prime set/retire/unset <seat>`; `role set <seat> <role>` / `role unset <seat>` | `baton-define`, `baton-list`, `baton-show`, `baton-request`, `baton-grant`, `baton-return`, `baton-reclaim`, `prime-set`, `prime-retire`, `prime-unset`, `orchestration-role-set`, `role-unset` |
| POST `/v1/role` | `role [<seat>] <role>` / `role [<seat>] --unset`; typed `{seat?, role, caller}` | `role-set` |
| POST `/v1/link` | `link <seat> [--parent <you>] --role <pm/worker/pa>`; argv + `caller` only | — (`crates/cli/tests/seat_roles.rs`) |
| POST `/v1/report` | `report now/state/question/blocked/clear/verify`; state metadata flags below | `report-question`, `report-verify` |
| GET and POST `/v1/anomalies` | `anomalies [--here] [--project <slug>]`; GET `here=<absolute-path>&project=<slug>` | `anomalies-list`, `anomalies-argv` |
| GET and POST `/v1/decisions` | `decisions [--state <open/answered/all>] [--asked_by <seat>] [--parent <seat>]`; GET keys `state`, `asked_by`, `parent` (literal underscore; `--asked-by` refuses) | `decisions-list`, `decisions-argv` |
| POST `/v1/answer` | `answer [--supersede] <decision> <answer>`; typed `{decision, answer, supersede?, caller}` | `decision-answer`, `decision-answer-supersede` |
| POST `/v1/close` | `close <seat> [--reason <text>]`; typed `{seat, reason?, caller}` | `seat-close` |
| POST `/v1/reap` | `reap [--dry-run]`; typed `{dry_run?, caller}` | `reap-dry-run` |
| POST `/v1/spine` | `spine append --kind <kind> [--refs <a,b>] [--project <slug>]`; `events/render [--since N] [--peer <seat>] [--project <slug>]` | `spine-append`, `spine-events`, `spine-render` |
| POST `/v1/adopt` | Existing identity admission plus optional asserted role; omitted parent and role remain unsaid | `adopt-with-role` |
| POST `/v1/register` | Existing verified registration plus optional asserted role; failed admission cannot commit a role | `register-with-role` |

The case-id column names exact `.routes[].cases[].id` entries in [`governance-routes.json`](../../crates/testkit/fixtures/golden/api/governance-routes.json): select the row's method/path, then that id for its pinned `request` and `response` (and `shim_request` when supplied). For combined GET/POST rows, `*-list` is GET and `*-argv` is POST. A GET fixture's `request:null` means no request body. The existing report now/state event payload examples are separately pinned by `report-now` / `report-state` in [`governance-events.json`](../../crates/testkit/fixtures/golden/api/governance-events.json); the new report route cases are question/verify. These pointers are fixture provenance, not new captures.

`--json` is accepted by the command surfaces. Read families share their GET and POST parser/projection; GET `here=true` cannot guess the caller's folder and refuses. Exact peer/project filters do not widen scope. `status-stale` is node-keyed, so a project filter can omit it: supervisors query anomalies unscoped before declaring cards fresh.

**Seat roles (plan 166).** Every setter enforces the closed vocabulary `prime | pm | worker | pa` and refuses anything else (`E-RS-ARG`, naming the allowed list). This covers `role`, `orchestration role set`, `register`/`adopt --role`, spawn and link. Roles are stamped by a governor, never inferred or backfilled. The placement verbs accept only `pm | worker | pa`; `prime` comes from designation.

- `POST /v1/spawn` with `role` resolves `caller`. That seat becomes the parent and `assigned_by`; a different `parent` refuses `E-RS-OWNERSHIP` before launch. The seat row, `seat.put`, the `seat_roles` row and `role-set` commit in one transaction.
- `POST /v1/link` lets the caller take a live seat whose recorded parent is absent or not live, or re-role a seat it already parents. A parent change appends `seat.put`, a role change appends `role-set`, both in one transaction, and an unchanged role appends nothing.
- Link refusals:
  - `E-RS-OWNERSHIP` with `details.parent` for a live foreign parent;
  - `E-RS-OWNERSHIP` with `details.reason:"prime"` for a prime;
  - `E-RS-ARG` with `details.reason:"cycle"` for the caller's own ancestor.
- The receipt carries `seat`, `parent`, `previous_parent`, `role`, `assigned_by`, `assigned_at`, `parent_changed`, `role_changed` and `seqs`.
- Self-asserted `adopt --role` is unchanged, a known divergence from TS. Placement needs the SQLite registry: fake-registry daemons refuse it rather than desync.

Project/stream/fence/dispatch/task records are rs store authority, not files under `~/.pij/`. Fences describe intended writes, not permission. Stream close changes its record, not the worktree. Dispatch persists packet digest and outbound linkage before delivery; queued is not delivered, and delivered is not acknowledged. Only the resolved recipient can ack the matching packet SHA; identical ack is idempotent. Canary requires actual nonce-correlated dispatch/ack and observed runtime/model evidence, not descriptor presence. `attest --plan-id` never grants native-extension-delivery attestation. `node show` joins rs records only. `spine render` returns `{text,cursor}` and never writes a legacy ledger file.

Source: [`governance.rs` parser and handlers](../../crates/daemon/src/http/governance.rs), [`report.rs`](../../crates/daemon/src/http/report.rs), [`anomalies.rs`](../../crates/daemon/src/http/anomalies.rs), [`decisions.rs`](../../crates/daemon/src/http/decisions.rs), [`lifecycle.rs`](../../crates/daemon/src/http/lifecycle.rs).

## Canonical wire examples

The following excerpts are verbatim canonical fixture objects (`role-set`), not new live captures. Send typed role fields OR the fixture's shim argv form, never both.

POST `/v1/role` request:

```json
{
  "seat": "pij-worker",
  "role": "pm",
  "caller": {
    "TMUX_PANE": "%10",
    "cwd": "/work/project"
  }
}
```

Complete success envelope:

```json
{
  "ok": true,
  "command": "pij role",
  "v": 2,
  "data": {
    "seat": "pij-worker",
    "role": "pm",
    "assigned_by": "pij-parent",
    "assigned_at": 1788739200000,
    "seq": 101
  }
}
```

Matching canonical event frame (decode `event.payload` as a JSON string):

```json
{
  "type": "event",
  "machine": "fixture-machine",
  "cursor": 101,
  "event": {
    "v": 1,
    "at": 1788739200000,
    "kind": "role-set",
    "seat": "pij-worker",
    "payload": "{\"actor\":\"pij-parent\",\"action\":\"assigned\",\"record\":{\"seat\":\"pij-worker\",\"role\":\"pm\",\"assigned_by\":\"pij-parent\",\"assigned_at\":1788739200000}}"
  }
}
```

## Report assignment metadata

| Leaf / flag | Contract |
|---|---|
| `report state <state> --assignment <id> [--refs a,b]` | Assignment must exist and belong to the reporting caller. |
| `report blocked\|question "<text>" --assignment <id> [--refs a,b]` | Same ownership check, before state publication or durable question creation. |
| `--refs a,b` on `state`, `blocked`, `question` | Allowed without an assignment; split on commas, trim entries, discard empty entries, preserve order. |
| `report verify <seat> [--assignment <id>]` | Unchanged: current parent verifies matching task-scoped done evidence; self-verification refuses. |
| `--project` | Refused: rs reports are node-scoped; project comes from the node (`pij node show`). |
| `--for` | Refused: rs has no relay authority yet; report as yourself. |

Unknown task ids return `E-RS-ASSIGNMENT-UNKNOWN`. Another node's task returns
`E-RS-ASSIGNMENT-NOT-YOURS` and names its owner. Both are HTTP 400 v2 refusals,
with the code in `details.code`; neither writes state or opens a question.
Assignment and refs flags require one nonempty value; duplicates refuse.
`now` and `clear` do not accept assignment/refs flags.

The canonical persisted and projected names are **`assignment_id`** and **`refs`**.
Old `report.state` records read with `assignment_id: null` and `refs: []`.
`node show` exposes the selected node's latest record at `data.node.state`
(null before its first declaration); child summaries remain structural.
`pij state` exposes `assignment_id` and `refs` beside `stateNote`, and report
JSON receipts expose the same metadata. An unscoped declaration or `clear`
replaces previous metadata rather than retaining stale task context.
Reporting `done` never closes a task; closure remains an explicit task operation.

[`report-assignment-client-parity.json`](../../crates/testkit/fixtures/golden/api/report-assignment-client-parity.json)
retains isolated native/shim node/state readbacks and assignment refusals.
Stdout is byte-equal; existing refusal exit policies remain native `2`, shim `4`.
The deterministic state-record golden is separately generated from the core
service; neither fixture claims production deployment.

## Questions, answers and current parents

`report question <text>` creates a durable decision plus `decision.opened` atomically; `question_seq` is that actual allocated event sequence. A later report clear/state change does not answer or delete the question. Read projections and parent filters follow the asker's **current parent**; the historical `decision.opened` payload retains parent-at-ask. Root questions may be visible through prime fallback, but that visibility does not grant answer authority.

The asker or current parent may answer. Another-seat answers retain stable `answer_msg_id` and use the existing DeliveryService to the **asker**, not its parent. Durable delivery acceptance closes the decision; a rejection leaves inspectable open intent. Identical retry must not duplicate a push. Self-answer closes with `answer_msg_id: null`, without a redundant self-send. `report verify <seat>` is current-parent-only, never self-verification. With `--assignment`, it selects the latest done naming that assignment even if unrelated done events followed; without it, the latest done overall. A newer done for the same assignment reopens its unverified-done finding.

```bash
pij report question 'Who owns the migration?'
pij decisions --parent <current-parent> --json
pij answer <decision> 'The store unit owns it' --json
# Recovery ONLY after observing a terminal, non-delivered prior answer:
pij answer --supersede <decision> 'The replacement ruling' --json
```

Only the current parent can supersede, not the asker, former parent or unrelated seats. Eligible prior intent is failed/no-queue preparation or an already-recorded terminal non-delivered outcome (including expired-unknown/refused); elapsed time is not proof of expiry. Pending/running jobs, held outcomes and pre-injection reservations without a completed audit refuse `E-RS-ANSWER-IN-TRANSIT`. **An answer in transit cannot be withdrawn; wait for its terminal outcome.** Claim expiry returns to pending, not terminal expired-unknown. Delivered/acknowledged answers refuse `E-RS-ANSWER-DELIVERED`, including durable outcome evidence after bounded delivered-id eviction. No queue cancellation, withdrawal, new transport or custom retry is supplied.

Supersession atomically records the prior answer under `superseded` and opens fresh prepared intent. The new message id derives from the allocated supersession event seq; an obsolete completion cannot close that replacement. This requires queue=real and spine=real **sharing the decision/event SQLite authority**, not just matching labels. Mixed or fake combinations refuse HTTP 409 `E-RS-ANSWER-AUTHORITY-SPLIT`, naming `queue_backend` and `spine_backend`, before any replacement mutation/admission. Ordinary questions, answers, identical retry and self-answer remain available for supported configurations.

## Baton leases

```bash
pij orchestration baton define merge --resource branch:main --repo <repo>
pij orchestration baton request merge --purpose 'Integrate reviewed work' --pin <sha> --evidence 'gate receipt'
pij orchestration baton grant merge --to <request-id> --json
pij orchestration baton show merge --json
# Holder returns the EXACT lease id observed in grant/show:
pij orchestration baton return merge --lease-id <observed-lease-id> --evidence 'integration complete'
# Keeper reclaims that EXACT observed lease only after explicit judgment:
pij orchestration baton reclaim merge --lease-id <observed-lease-id> --evidence 'holder confirmed gone'
```

Return/reclaim **require** the observed `--lease-id`. Missing or stale ids return `E-RS-LEASE-STALE`, naming the current lease and remediation; a shim must not resolve a fresh lease on the caller's behalf. One holder, request purpose, pin re-verification and authorization remain enforced. An asserted role is not a lease grant. These are shared-store records/events, not legacy atomic lease files. The book can annotate evidence; it is not another lease authority.

`baton request` persists the request and publishes `baton.requested` before calling the existing delivery service, from the requester to the definition's `created_by` keeper. Its complete response includes `data: {request, seq, notice}`. The exact legacy notice text is pinned by `baton_notice_body_template` in the stable route corpus.

`notice` is `queued` for OMP extension-stream admission and for Pi extension-stream admission until ReaderRead; `delivered` requires actual transport confirmation. `unverified` uses existing `Dead`/`Recycled` process-incarnation evidence, not a new heartbeat-age rule. A missing or dissolved keeper yields `notice: null` and no send. The eight retained host cases keep their message/no-write counts; idle Pi's expected value was explicitly corrected from delivered to queued under the honest-receipt rule.

Send/probe errors or Held/Refused transport outcomes cannot be presented as success or erase the committed request. `E-RS-PARTIAL` retains `committed`, `event_published`, `request` and `seq` evidence. A publication failure prevents the notice send. A later reader acknowledgement may prove delivery without creating a second notice message; self-send still uses the existing refusal path.

## Retirement and conservative reaping

`pij close <seat>` is a self-or-recorded-parent tombstone, **not process termination or pane teardown**. The active roster excludes tombstones; history retains the reason/sequence. A parent repeat-close returns existing evidence. Retired self identity still refuses, naming tombstone evidence; it is not an authentication exception. There is no `--force` bypass.

```bash
pij reap --dry-run --json
# Only an authorized operator deciding to reconcile records runs the mutation:
pij reap --json
```

Eligibility: confirmed process death **or** `proc_start` mismatch, **and**, for a pane-bound record, confirmed absence of that exact pane. A pane whose root process started **after** the seat's own process is a different pane reusing the id, as after a reboot, and counts as absent (`pane: "reincarnated"`). That narrows the brake without adding policy: an older or unobservable pane root still vetoes. Paneless records require the confirmed stale process alone. A live/recycled process is never signalled. Unknown process/pane evidence is reported as `unverifiable` and blocks retirement. Removing that check can only permit the same set or more reaps: it is a **brake**, not retention policy or an age threshold. Dry-run changes neither records nor spine. Execution re-observes and compares the binding before atomic tombstoning so a concurrent revive is not retired from an old snapshot.

The process adapter uses `LC_ALL=C ps -o lstart=PIJ_LSTART -p <pid>`. Absence is specifically exit **1**, empty stderr, valid UTF-8 and **one header line whose trimmed value is `PIJ_LSTART`, with no subsequent line**. The canonical example is `PIJ_LSTART\n`. Exit 0 requires the header plus exactly one parseable process row. Empty stdout, wrong header, an extra blank/data row, any stderr (even whitespace), other exits, signals and parse errors are unknown—not dead. Existing regression names include `exit_one_with_header_only_and_empty_stderr_is_absent`, `successful_probe_without_process_row_is_unknown`, `failed_probe_requires_exact_header_only_output`, `any_stderr_vetoes_both_absent_and_live_protocols` and `other_exits_and_signals_with_header_only_are_unknown`; naming them is not a claim that this docs change ran them.

Sources: [`reaper.rs`](../../crates/daemon/src/reaper.rs), [`proc.rs`](../../crates/harnesses/src/proc.rs).

## Death sweep

The daemon automatically applies the same conservative reap rule every **5 seconds** by default; set `PIJ_RS_DEATH_SWEEP_MS` on the daemon to change the interval in milliseconds. `pij-rs reap --dry-run --json` exposes the same candidates for an equivalent observation. A candidate needs a `Dead` or `Recycled` process incarnation and confirmed absence of its recorded pane (paneless records use the existing stale-process rule). Unknown evidence is not death.

Each unchanged candidate receives one atomic `seat.tombstone` with `reason: "observed-dead"` and observation fields `pid`, `proc_start`, `pane`, `pane_present: false`. Already-tombstoned rows are not retired again. The sweep never signals a process or tears down a pane.

After tombstoning, the daemon pushes an obituary to the child's **recorded parent** through `DeliveryService::send`. It uses `pij_core::BG_ACTOR`, the shared daemon-owned `pij-bg` virtual actor already used for background-job completion—not a second sender identity. The exact text is pinned as `death_notice_body_template` in the [stable route corpus](../../crates/testkit/fixtures/golden/api/governance-routes.json):

```text
[pij] seat <id> died (<reason>) — pane <pane> absent, pid <pid> gone at <iso>. It was your child; revive with `pij-rs revive <id>` or leave it.
```

A send receipt proves only its recorded delivery strength: queued is not parent observation. Parents need not poll for child deaths. A parent that is tombstoned or itself observed-dead in the same sweep receives no notice; the daemon counts these suppressions in one line, `death sweep: N notice(s) withheld — recipient dead too`, rather than flooding dead parents after a reboot.

Obituary preparation, send and audit failures are isolated per seat: log the error, continue with the remaining parents, then report `death sweep: N notice(s) failed` once. A failed obituary does not roll back retirement or gain an automatic retry.

## Revive

```bash
pij-rs revive <seat> [--session <tmux-session>] [--name <window-name>] --json
# Only the recorded parent, or a prime for a parentless target:
pij-rs revive <seat> --assume-dead --evidence 'Observed stale binding; describe the evidence' --json
```

Native revive accepts an explicit tombstoned id **or** a seat observed `Dead` with its recorded pane absent; no preliminary `close` is needed. In the observed-dead case it first commits `seat.tombstone` with `reason: "revive-observed-dead"`, then relaunches under the same spawn-lock critical section. The receipt's `details.seq` cites the tombstone sequence.

A tombstone row is sufficient even when an older native retirement left no matching `seat.tombstone` event. Before relaunch, the daemon records `revive.legacy-tombstone` with the stored retirement timestamp and reason; `details.legacy_tombstone_seq` cites this audit and `details.seq` is omitted rather than inventing a tombstone receipt. A tombstone event from a previous incarnation does not count as a receipt for the current row. Audit publication failure leaves the tombstoned row untouched.

For `Recycled` (the pid belongs to another process incarnation) or `Unknown` (for example, the process probe is unavailable or no process binding is recorded), `--assume-dead` requires nonempty `--evidence`. The daemon resolves the caller from caller evidence: only the seat's recorded parent may override, or, **if the target has no parent**, a caller whose authoritative role is `prime`. An unrelated prime cannot override a parented seat. The override records `revive.assumed-dead` with the supplied evidence and observed binding, cited by `details.assumed_dead_seq`; it **never signals the replacement pid**. An `Active` process or a present recorded pane still vetoes revival, even with this flag.

**Revive resumes the conversation (plan 156).** It relaunches with the recorded `harness_session` in the harness's own spelling: `claude --resume <id>`, `copilot`/`omp --resume=<id>`, `pi --session <id>`, or `codex resume <id>`. The pre-bind row keeps that `harness_session`, so no later registration can relabel the conversation. If the conversation is already running elsewhere, it refuses with `E-RS-REVIVE-CONVERSATION-LIVE` and names the pane, before any write: "adopt there instead". "Elsewhere" means another seat holding the conversation with a live process, or, for Claude, a live process's own session record with a verified start. A seat with no recorded conversation refuses with `E-RS-ARG`, and `--fresh` is the only way to launch a new, blank conversation.

```bash
pij-rs revive <seat> --fresh --json   # explicit blank relaunch
```

Without an eligible override, `E-RS-REVIVE-LIVE` names the observation and exact override command; its remediation does not grant authorization. Shim `pij revive` still refuses `E-RS-UNPORTED`, and native `--print`/`--attach` remain unsupported.

## Event stream and cursors

`GET /v1/events` is NDJSON, **not SSE**. The first line is the fixture Hello below, not `type=hello` and not a v2 command envelope:

```json
{"build":"pij-rs 0.1.0","hello":true,"v":1}
```

An Event frame has `type`, `machine`, `cursor`, `event`; inner Event has `v:1`, epoch-millisecond `at`, literal `kind`, optional `seat`, and **`payload` is a JSON STRING**. Decode the outer line, then JSON-decode that string once. Inner `event.seq` is not serialized: advance the appropriate machine's cursor from the frame. The canonical `role-set`/governance payloads use `{actor,action,record}`; seat.put decodes directly to the persisted SeatDescriptor and seat.tombstone to its reason object. Existing report.now/report.state payloads are not renamed. New publications have real timestamps; historical `at=0` rows are not rewritten.

No `since` means live-only. Replay takes a URL-encoded JSON cursor map such as `{"fixture-machine":100}`, exclusive per machine; an integer `since=100` refuses. Native CLI spelling: `pij-rs tail --since fixture-machine=100` (repeat `--since` for each machine). This follows daemon events, **not a peer transcript**. `pij spine events --since 100` is a separate local-spine query and returns its local cursor.

All producers append through the sole EventBus/store ordering seam, commit before broadcasting the returned seq, and preserve replay/live boundary ordering without duplicate/skipped rows. Caller cancellation after admission does not abandon commit/broadcast; shutdown joins admitted producers before flush. Destructive governance mutations commit state and spine event atomically or neither. Consumers must not advance a cursor past an unemitted row.

## Unsupported status

This anchor is permanent; refusals and shipped consumers must not depend on an active plan directory. The inventory distinguishes **shim gaps** from native capability. A missing route never enables legacy execution, even when explicitly forced, and a family row does not promise every old leaf or flag.

- **Shim-only gaps / different meanings:** `spawn`, `revive`, `tail` and `daemon` refuse `E-RS-UNPORTED`; native capability is not a grammar-compatible shim port. Use the documented native forms in the [peer](../../skills/pij/references/routes/peer.md) and [ops](../../skills/pij/references/routes/ops.md) routes where available. Native tail is an event stream, not a transcript. [Native revive](#revive) accepts tombstoned or observed-dead ids and the authorized `--assume-dead --evidence` override; `--print` and `--attach` remain unsupported. Native spawn has no legacy layout/task/branch/plan-id flags.
- **List/sessions filters:** shim `pij list` forwards declared `harness`, `folder`, `parent` and `scope=local` query values to GET `/v1/seats`. Shim and native `pij-rs list --here` scope to caller cwd; path-valued `--here` refuses. Native list has no other filters. `pij sessions` routes GET `/v1/shim/sessions` with no query flags or legacy union. Unsupported `--role`, `--prime`, `--archived` and tree semantics refuse instead of being dropped. Prime designation has no dedicated list/getter projection here: use actual designation receipts/events and authoritative government/human evidence; role assertion or an empty filtered view cannot prove absence.
- **Baton projection limits:** a blocked-time field is not provided. Automatic request notices and their honest delivered/queued/unverified/null projection are supported as described under [Baton leases](#baton-leases); a durable request alone still does not prove recipient observation.
- **Unported command surfaces:** `agent`, `path`, `telegram`, `models`, `watch`, `unwatch`, `chore`, `watchdog`, `focus` and `tree` refuse through the shim. This does not remove already composed sidecar internals; it does not advertise those old administrative grammars as a native port. `link` is ported (plan 166) with a narrower grammar: the caller is always the parent, and `--role` is required.
- **Inbox/admission:** verified external `inbox register` and `inbox --wait [ms]` are [supported](#verified-paneless-external-admission), not gaps. Paneless adopt, unlisted inbox leaves, generic shim `register`, send `--wait` and attachment semantics remain refused. Never fabricate host process evidence or use a legacy fallback; extension-owned registration/receive stays owned by that extension.
- **Control and bg are supported, not gaps:** remote compact/new/reload, compact-self and bg create/list/tail/kill remain shipped. Controls carry no body; acceptance is not execution. Copilot and paneless controls refuse; new/reload require self or recorded parent plus target arming, not a prime role. Never replace refusal with slash text or sendkeys. bg remains daemon-owned detached execution (in the caller's cwd unless `--cwd`; optional `--timeout` ends it with a TIMEOUT turn; `list` shows running time and duration; `--events` makes an event source whose child fires `pij bg emit` / `POST /v1/bg/{job}/emit`, authenticated by its per-job `PIJ_BG_TOKEN` outside the daemon-key ring, batched per `--min-interval`/`--inline-max`, held as FYIs with `--fyi`, and routed to a warm prime or Telegram instead of waking a cold owner) with durable completion injection, bounded server-side log reads and owner/parent authorization; it introduces no answer queue cancellation.
- **Native commit-trailers data gap:** forwarding is supported, but `commit_trailers::run` currently calls `derive` with no repository-designation input and does not query current assignment. It reads role-joined self/local seats, then uses a complete recorded-parent root as the explicit interim fallback. `Pij-Plan` derives from worktree path, branch or local flow context. Do not claim designation/current-assignment lookup or use stale legacy rows to fill either gap.
- **StateCard:** inspect its `unsupported` notes instead of manufacturing missing telemetry/lifecycle fields. Optional/null role is absence of assertion, not proof that the role port is unavailable.
- **Recovery boundary:** `answer --supersede` is terminal-only and shared-real-authority-only as above. The refusal is intentional; no queue withdrawal exists. Close/reap reconcile records, not processes. An unsupported force flag cannot bypass ownership.
- **Native sessions capability:** there is no `pij-rs sessions` subcommand in this cutover. The shim's session projection is compared with original live GET `/v1/shim/sessions` response bytes, not an invented native CLI surface.
- **Registration CLI boundary:** `register --role` remains native/HTTP-only. Shim `register` refuses `E-RS-UNPORTED` by name; it is not a fallback registration path.
- **Exit-policy convergence:** native and shim refusal exit policies should converge in separate work. Until then preserve the table above; do not silently align statuses while changing JSON routing.

The actual CLI-verb table is [`skills/pij/SKILL.md`](../../skills/pij/SKILL.md); routing enforcement reads that table rather than a test-owned duplicate. Source inventories: [shim route/refusal declarations](../../.omp/extensions/pij/core/generation-routing.ts), [native Command enum](../../crates/cli/src/main.rs), [native attribution helper](../../crates/cli/src/commit_trailers.rs), [inbox parser](../../crates/daemon/src/http/shim.rs), [identity admission](../../crates/daemon/src/http/identity.rs). Final isolated runtime, full-envelope byte parity and production/chainglass evidence belong to the PM/prime, not this document.
