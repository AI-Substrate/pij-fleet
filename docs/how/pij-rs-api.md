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

`claim`, `claims`, `delivered` and `directory memory` (plan 209) also pass through: `pij` runs `pij-rs` for them, so the shim prints the native text (one line per claim, the delivered edits, the memory INDEX), the real `--help`, and reads `--body-file -` from stdin.

`fleet-report` (plan 162) is another native pass-through: `pij fleet-report <FOLDER>` forwards to `pij-rs fleet-report`, which folds the project's transcripts in-process through Unisphere, reads seats from the store **read-only** (never created, never migrated) and writes `index.html`, `report.json`, `report.js`, `manifest.json` and `tables/` to one folder. It never contacts the daemon. P1 refuses `--format parquet` (`E-RS-FLEET-FORMAT`) and `--prep-target` (`E-RS-FLEET-PREP-TARGET`) by name; the output folder carries names and paths unless `--anonymise`.

Known limit (plan 169): fleet-report prices every call at the default table; router sessions are not modelled per served model.

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
| `directory`, `claim`, `claims`, `delivered`; `send` to a `role:` address, with `--to`, or `--help` ([shim forwarding](#shim-forwarding-plan-213)) | Native command status | Same native command status |

Sources: [`exit_code`](../../crates/cli/src/lib.rs), [shim refusal/output handling](../../.omp/extensions/pij/cli.ts). The native-pass-through signal contract is also preserved; this table does not redefine OS signal termination.

### Shim forwarding (plan 213)

The `pij` shim ([`pij-cli.cjs`](../../harness/scripts/pij-cli.cjs)) hands pij-rs's community verbs to `pij-rs` before its TypeScript router loads: argv and stdin unchanged, and pij-rs's own stdout, stderr, help and exit code. Nothing is re-rendered or wrapped, so `pij <verb>` and `pij-rs <verb>` print the same bytes. The verbs are listed once, in [`pij-native-verbs.json`](../../harness/scripts/pij-native-verbs.json): exactly the pij-rs subcommands that own their grammar and help (clap's help flag disabled), today `directory`, `claim`, `claims`, `delivered`, `output`, `outputs` and `requests`. `send` goes too, from a tmux pane, when its recipient is a `role:` address or it uses `--to`, and from anywhere for `--help`; `pij-rs send` takes the shim's positional form (`send <recipient> [<text>]`) as well as `--to`/`--body`. The shim finds the verb past pij-rs's global options (`--json`, `--state-dir`, `--addr`) and reads each send flag with its value, from value-flag tables in the same file, so a value that looks like `role:…` is never taken for the recipient. Sends to a seat, every paneless send (the router registers the observed host first and never trusts an inherited `PIJ_SESSION_ID`), the shim-only `--file`/`--caption`, and any explicit `PIJ_DAEMON_GENERATION` other than `rs` keep the routed path. `pij-rs` is spawned by absolute path, from the first absolute PATH entry that holds it (a relative entry is skipped, as the ddocs resolver does); with none, the shim exits 127 and names it. Native `send`, `compact-self` and `inbox` resolve the acting seat with the full caller context, so the daemon applies the same harness-session checks as on the routed path.

Tests: `shim_forward_tests` in [`crates/cli/src/main.rs`](../../crates/cli/src/main.rs) holds the verb list equal to clap's verbs that own their help and the value-flag tables equal to clap's options (with new and stale entries as negative controls), and [`crates/cli/tests/shim_forward.rs`](../../crates/cli/tests/shim_forward.rs) runs the real shim: help byte-equal to `pij-rs`, and every flag pij-rs's help names reaching a recording `pij-rs` with argv, stdin, output and exit code unchanged, with the kept-path calls as negative controls. Cost: one `node` start and one `pij-rs` exec per call, where the routed path started node twice (the second through tsx); measured medians at load 36 were 0.085 s against 0.546 s (`claim --help`) and 0.115 s against 0.639 s (`claims`). Nothing runs per tick, and the daemon launches nothing new.

## Store health and daemon logs

`GET /health` probes the shared status store's schema and acquires then rolls back
a write transaction; a responsive HTTP listener alone is not healthy. HTTP 200
has `ok:true`, `data.status:"healthy"` and
`data.store:{status:"healthy",timeout_ms:2000}`. `data.spawns:{total,by_op}` counts
the processes this daemon has launched since it started, by `<program>.<site>`
operation (`ps.proc-liveness`, `tmux.send-keys`, `git.orchestration-repo`, ...); the
`by_op` values sum to `total` (plan 180). Delivering to a Claude seat launches
nothing: the socket transport reads the peer's start in-process, once per
resolution, and `deliver` reuses the endpoint its own probe resolved after a fresh
start re-read (plan 218, retiring `ps.uds-peer-start`). `data.pid` is the daemon's own process
id (plan 190): the spawn counter resets on restart, so two readings compare only
when their pids match. `data.registrations:{total,ps,by_key,by_seat}` counts registration
attempts since start (plan 211, issue #145): `by_key` is per
`<path>.<mode>.<outcome>` with `{count,ps}`, where `ps` is the ancestry `ps`
reads those attempts paid. `path` is the client's advisory `attempt` on
`/v1/register` (`explicit`, `receive`, `heartbeat`, `shim`, `lease-retry`;
`unspecified` when absent, `other` when unknown) or the daemon caller (`adopt`,
`claude-bind`, `codex-bind`); `mode` is `pane` or `paneless`; `outcome` is
`new`, `rebound`, `unchanged`, `refused`, `retired`, `retryable`,
`session-hold`, `holder-current` or `error`. Every parsed `/v1/register`
counts once, including one the route refuses before registration (an invalid
role, a pane it cannot observe). `by_seat` is per address: the resolved seat, or
for a refusal the claimed id when it is a seat (`(unnamed)` when empty);
addresses that are not seats, longer than 64 bytes, or beyond the first 64
count under `(other)`. Each attempt's `ps` tally closes when the attempt is
counted: an observer a timeout left running may not launch afterwards, so no
read goes uncounted. The counters are in-process (one mutex-guarded pair of
maps): no launch, event or store write per attempt, and nothing reads them to
decide anything. `data.ddocs` is the ddocs path the daemon resolved once at
start (`PIJ_DDOCS`, which must be absolute, else PATH), or `null` when it found
none; `just doctor` prints it. A relative or empty PATH entry is taken relative
to the daemon's directory at start, so the kept path is always absolute. `data.gh` is
the gh `pij delivered` launches, resolved the same way from `PIJ_GH`, else PATH,
or `null` (plan 196). A failed or exhausted probe returns
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

Every `/v1` route requires a bearer key. The key grants **machine access**, not a seat identity or authority over another seat. The per-boot local key (`<state-dir>/daemon.key`) reaches every route. A **paired machine's** key (from [`peers.toml`](pij-federation.md)) authenticates as that machine's alias and reaches only `POST /v1/send`, `GET /v1/seats?scope=local` and `GET /v1/events?scope=local`; every other route, and those two without `scope=local`, answers HTTP 403 `{error:"auth", meta:"E-RS-PEER-SCOPE: …", details:{code:"E-RS-PEER-SCOPE", machine}}`. The allowlist is `Endpoint::peer_access` in [`http/mod.rs`](../../crates/daemon/src/http/mod.rs), so a new route is refused to peers until that function names it. Callers are within that trusted-machine boundary; `CallerContext` is not a cryptographically authenticated per-seat credential. The daemon resolves the caller using its existing identity service and observed pane/process binding. An asserted `PIJ_SESSION_ID` locates a record; it cannot create a seat or override contradictory pane evidence. Caller-supplied pid/start fields in the shim context are diagnostic, not adoption evidence. Never substitute a short-lived CLI pid for the harness process.

Governance families accept `{ "argv": ["project", "list"], "caller": { "TMUX_PANE": "%10", "cwd": "/work/project" } }`. `argv` includes the family; a mismatched route/family refuses. Native typed role/answer/close/reap requests converge on the same daemon operation as shim argv, not an independent semantic parser. Do not mix typed operation fields with argv. `--actor`, `actor` or `assigned_by`, where accepted, must match the resolved caller: they never grant impersonation.

`RoleService` is authoritative: asserted roles live in `seat_roles`, joined into whoami, `/v1/seats`, StateCard and phonehome. A descriptor's stale role does not win. The projected role is optional (`Option<String>`): a string is an assertion; JSON `null` means no asserted role, never an inferred `worker` or empty string. Consumers must preserve null versus omission and must not reinterpret an omitted role as a request to clear it. Adoption/registration with role omitted preserves an existing assertion; a fresh unstamped seat remains null. Explicit null on adopt/register is not an unset operation. Use `pij role <seat> --unset` or typed `/v1/role` with `"role": null`; omitting role from that typed mutation is invalid.

Self or the target's **current recorded parent** may assert/unset its role; an unrelated seat gets `E-RS-OWNERSHIP`. Protocol may require governors to designate roles, but the service's self-or-parent contract is explicit. An asserted display role, even `prime`, confers no general governance or remote-control authority. Narrow exceptions are [assume-dead revive of a parentless seat](#revive) and [parking a blocked extension inbox head](#extension-inbox-recovery), which accept an authoritative `prime` role. Prime designation is a separate orchestration record.

Sources: [`identity.rs`](../../crates/daemon/src/http/identity.rs), [`role.rs`](../../crates/daemon/src/http/role.rs), [`types.rs`](../../crates/daemon/src/http/types.rs), [`governance.rs`](../../crates/daemon/src/http/governance.rs).

## Seat authority

The daemon key proves machine access, not which seat is calling (F88, PRD 003 req-10). [`harness/scripts/route-inventory.json`](../../harness/scripts/route-inventory.json) classifies every (method, path) pair the daemon serves:
- **machine-public**, with the reason, for any key holder;
- **seat-authority**, which acts with a seat's authority. Each entry names its class (send-as, inbox-claim, role-and-holder, outputs, activity, registration, controls, adoption, attribution), the identities it grants authority by (seat, parent, prime, holder, owner, recipient), and today's guard. The guard codes are the inventory's own: R resolver, B hook binding, N native tuple, S shim caller, C control admission, P provenance exception, A asserted.
- **job-token**, for the background emit hook.

Today every seat-authority guard rests on identifiers a caller can present, so a local key holder can act as another seat; ruling C32 accepts that until req-10 lands. `just route-inventory-check` (a `harness checks` stage) probes the routes the router really serves and fails on a served pair missing from the inventory, a stale entry, or an entry without a valid classification, so a new route can't ship unclassified. How each seat incarnation will get a private capability, and the order phase 2 gates the classes in (send-as first), is decided in [plan 221's workshop](../plans/221-seat-provenance/workshops/001-capability-bootstrap.md).

## Spawn harness choice and retirement policy

**OMP and Pi are distinct harnesses, with no shared default.** Native `pij-rs spawn --harness <h>`, HTTP `POST /v1/spawn` and in-process `pij_spawn({harness: …})` require an explicit selection from `omp`, `pi`, `claude`, `copilot` and `codex`; the parent runtime is not a default. `--bin` (HTTP `executable`, in-process `bin`) overrides only the executable path, never selects the harness. Bare selectors such as `--harness pi --bin omp` refuse with the fix `--harness omp`; a known different harness basename also refuses inside an absolute path. A matching absolute path, such as `--harness omp --bin /absolute/path/to/omp`, is accepted. OMP uses provider-qualified model ids such as `github-copilot/gpt-6-astra`; Pi uses its own installed catalog's exact id, such as `gpt-5.6-sol`, with provider qualification when needed. Model availability must be established in the selected runtime, not inferred from the other.

| Harness | Required selection | Native model/effort translation |
|---|---|---|
| OMP | `--harness omp` or `"harness":"omp"` | `--model github-copilot/gpt-6-astra --effort high` becomes OMP `--model github-copilot/gpt-6-astra --thinking high` |
| Pi | `--harness pi` or `"harness":"pi"` | `--model gpt-5.6-sol --effort high` becomes Pi `--model gpt-5.6-sol:high` |
| Copilot | `--harness copilot` or `"harness":"copilot"` | `--model claude-opus-5 --effort high` becomes Copilot `--model claude-opus-5 --context long_context --effort high` |
| Copilot HydraFusion | `--harness copilot --model hydrafusion` (or `hydrafusion-max`) | becomes Copilot `--model hydrafusion --context default`; adding `--effort` refuses with `HydraFusion routes effort itself; omit --effort` |

HydraFusion ids (`hydrafusion`, `hydrafusion-max`) are Copilot's per-turn model router. Copilot refuses every other context tier for them ("HydraFusion models do not support context tiers"), and omitting `--context` would inherit a persisted `long_context` config tier, so pij passes the explicit `default` tier. Copilot also refuses reasoning effort for them, so pij refuses `--effort` before launch rather than dropping it. The `--model auto --auto-tier hydrafusion` form is refused by Copilot 1.0.97 ("requires dynamic Auto tiers"); use the model id. The seat's recorded model stays the synthetic id; the per-turn routed model is not reported.

These are argument-shape examples, not catalog or runtime receipts. `pij models` catalog enumeration remains unported (`E-RS-UNPORTED`): agents still cannot enumerate per-harness catalogs through pij, and no native catalog command is supplied. Consult the selected harness's installed catalog, then prove the choice with a canary and actual runtime evidence. Existing native launch flags are otherwise unchanged; native spawn still has no legacy layout/task/branch/plan-id flags.

**No modal questions (F110).** Every Copilot launch carries `--no-ask-user`, and every Claude launch carries `--disallowedTools AskUserQuestion`. That covers spawn, and revive too, since it relaunches through the same argv builder with `--resume`. A seat asks in its text, never in a dialog its harness draws (pij skill invariants 9-10). Codex, OMP and Pi have no such tool flag. Pinned by `copilot_and_claude_launch_without_their_modal_question_tool` in `crates/harnesses/src/spawn.rs`.

The daemon's `Config.retired_harnesses` reads comma-separated `PIJ_RETIRED_HARNESSES`. The generic default is empty: no harness is retired. This machine's `just bounce-rs` recipe explicitly sets `PIJ_RETIRED_HARNESSES=pi`; Pi is not hard-coded as globally retired. With that policy, native and in-process Pi spawn refuse with `harness pi is retired on this machine; use omp (or pass --allow-retired)`.

An intentional native `pij-rs spawn --harness pi --allow-retired …` sends HTTP `"allow_retired":true` to `/v1/spawn`. The daemon accepts the override with a spine event; inspect `pij spine events --peer <new-seat> --json` and retain that evidence. Omitted or false `allow_retired` does not bypass policy. The override does not rewrite the policy or change existing seats' registration, receiver ownership or caller authority.

The existing `GET /health` v2 response adds `data.retired_harnesses`, an array of harness names (`[]` for the generic configuration, `["pi"]` for this machine's recipe). In-process spawn reads this through `client.health()` before local pane or spawn-expectation mutation; it does not re-read `PIJ_RETIRED_HARNESSES` in the extension environment or infer policy from the parent harness. OMP and Pi use their distinct local launch paths; external harness choices use native `/v1/spawn`. Use native `--allow-retired` when deliberately overriding retirement, never a `bin` selector. `pij-rs spawn --role pm|worker|pa` (HTTP `role` plus `caller`; in-process `pij_spawn({role})` for claude/copilot/codex) stamps the child's role from above; see [Seat roles](#governance-route-inventory). In-process omp/pi spawn refuses `role` until pij-fleet#25.

A pane is not a worker (pij-fleet#44). After any successful launch, in-process `pij_spawn` polls `GET /v1/seats` every 500ms for a bound seat (non-null `proc`) whose `spawn_id` is this launch's. The spawn id is the only proof. Pane ids reset with the tmux server and `proc_start` has one-second resolution, so a pane or time match never counts, and neither does a row with another or no `spawn_id`. Every path carries the id:

- OMP and Pi children receive `PIJ_SPAWN_ID` and register with it. The parent mints it as `s-` plus 32 lowercase hex characters from 128 random bits, the daemon's own spawn-id shape. It uses no clock or counter, so two parents launching in the same millisecond never share one.
- Daemon-launched children bind to the `/v1/spawn` row, which already holds a daemon-minted id of the same shape.

One budget, `PIJ_SPAWN_REGISTER_TIMEOUT_MS` (default `20000`, read once per extension runtime), bounds the whole wait.

- Polling stops a quarter of the budget before the deadline (at most 1s). That last slice is kept for the pane check.
- Each roster read carries an abort signal for the polling time left, response body included. No read starts at or after that point.
- The one `tmux capture-pane` gets only the time left in the budget, and is skipped when none is left. The diagnosis is then `no-registration`.
- The wait therefore ends at the deadline, plus process-exit and timer slack.

A match reports `spawned pij worker <id>`. Otherwise the tool reports `spawned, NOT registered (<diagnosis>)` with the spawn id, the pane, the milliseconds waited and any roster read failure, plus a local `tmux capture-pane` command for inspection. `details.registration` is `{registered:false, waitedMs, diagnosis, rosterError?}`. The diagnosis is fixed: `boot-failed` when the child's own `pij: boot registration failed` notice is visible in its pane, otherwise `no-registration`. Pane text is never forwarded, in `content` or in `details`, because a pane can show credentials. The child's reason reaches the spawner through its own boot-failure message. The tool never claims plain success for a seatless pane, and it neither kills the pane nor retries the spawn.

## Registration and tombstone continuity

Native registration bounds each `ps` observation to **1 second** and the complete
host/ancestry observation to **5 seconds**. These are fail-closed resource brakes,
not identity policy; process-start sandwiches, executable and ancestry checks
are unchanged. Timeout refusals name the observed PID and return HTTP 409 with
`details.retryable:true`. Copilot extensions retry that existing typed response
with ordinary registration backoff; it is not a `native-session` Hold.

Every successful `/v1/register` returns and stores a live descriptor (`tombstoned_at: None`). An existing retired address is resumed only with the **same harness AND** at least one continuity witness: the same nonempty harness session (`HARNESS_SESSION_ID`, `harness_session` or `harnessSession`), the same nonempty `spawn_id`, or the recorded process incarnation corroborated by the daemon. Missing values and a matching pane alone prove nothing. Without continuity it returns HTTP400, v2 `error:"refused"`, with `seat <id> is retired (<reason>, <at>); a different session may not take it`, without changing the row; a different harness adds `; harness mismatch: row <h1>, claim <h2>` and is never adopted by a lift. Paneless Claude/Codex/Copilot session selection includes retired addresses, preferring live rows, then greatest `proc_start`, then ascending id; historical duplicates are not host owners. Their existing live-owner, session and pull-shape restrictions remain stronger than the continuity ladder. A lift clears both tombstone fields, keeps the address, harness, parent, authoritative role and Telegram bindings, updates the claimed process/pane, and emits exactly one resume event with `prior_reason`, `old_proc`, `new_proc`, `old_pane` and `new_pane`: `seat.native-resumed` for native Copilot, `seat.resumed` otherwise. Native ownership/HOLD checks, the verified spawn-prebind exception and the `revive` verb are unchanged.

That refusal carries the typed discriminator `details:{code:"E-RS-SEAT-RETIRED",seat:"<id>"}`, on `/v1/register` and `/v1/adopt` alike; match the code, never the prose (pij-fleet#44). `GET /v1/seats` omits retired rows, so an OMP or Pi extension choosing its own startup name can pick a retired seat's id. Only that self-chosen name advances to its next memorable candidate on this code, for at most 8 claims in total. An assigned identity never advances: `PIJ_SESSION_ID`, a reload's own id, a same-pane or same-process match, or a `/new`/`/fork` replacement keeps the refusal terminal. A boot that still fails notifies at error level, sets the footer to `boot failed` and, when `PIJ_ANNOUNCE_TO` or `PIJ_PARENT_ID` names a spawner, sends it one best-effort `pij boot failed (spawn <id>, pane <pane>): <reason>` message from the non-seat sender `pij-boot-failed-<pid>`.

A native Copilot claim that names an address recorded for a different native session is refused before the daemon observes the host (plan 211, issue #145): `E-RS-SEAT-RETIRED` when that address is retired, else the different-session refusal. The same check decides again under the registration lock, so this early read is a brake: removing it refuses the same claims later, after the ancestry `ps`. The native Copilot extension treats `E-RS-SEAT-RETIRED` on `/v1/register` as final: its receive and heartbeat loops stop and report `receive-held` once, instead of re-registering on a 5 s backoff.

An owner's close is final for continuity (F149). `pij close` records its reason as `owner-close` (or `owner-close: <reason>` with `--reason`), and a seat retired with that reason is refused on `/v1/register` and `/v1/adopt` even when its own session, spawn or process claims it, with `E-RS-SEAT-RETIRED` and the message `E-RS-SEAT-RETIRED seat <id> was closed by its owner; ...` (the code first, so a client that only prints the message can still match it). The row stays tombstoned. Without this a closed seat whose process was still running lifted its own close about a second later: the Claude status line's heal loop re-adopted the same conversation. Only `pij revive <id>`, which writes a fresh live row before it relaunches, and an operator's verified `adopt --reclaim` (a deliberate, checked operator action like revive) bring the address back. The store enforces this at the write, not only at the registration's earlier read: the seat upsert keeps an owner-close tombstone exactly as it is (neither cleared nor swapped for a liftable reason) unless the write is revive's or reclaim's, and otherwise writes nothing and refuses with `E-RS-SEAT-RETIRED`. So a registration that read the seat live, then lost the race to its owner's close, is answered as closed and the close stands (review of #200). A close of a seat already retired for another reason (`observed-dead`, `reaped`) records it again as `owner-close`; a repeat close of an owner-closed seat returns its existing receipt. Every other tombstone reason (`observed-dead`, `reaped`, a pre-F149 free-text close reason) keeps the continuity lift. The Claude status line keeps a `.closed` marker for the session after this refusal, shows `closed: <seat>` and stops adopting until the conversation changes; OMP re-registers only on a re-attach notice, so it is refused once rather than in a loop.

## Adopt continuity order (plan 156)

`/v1/adopt <pane> --harness <h>` picks the seat in this order:

1. **Operator reclaim** (`--reclaim <seat>`, below), when it is supplied.
2. **The conversation's seat.** A seat follows its conversation, live or tombstoned, if it has the same harness. A tombstone lifts with one `seat.resumed`, and the pane's previous registration for this same process is superseded. For **Claude** the daemon derives the session on every adopt from the pane process's own `<claude home>/sessions/<pid>.json`, across every Claude home including `~/.claude-alt`. The record counts only when its `pid` is the process tmux reports and its `procStart` matches that process's observed start, so named resumes resolve too. `--harness-session <id>` is only a cross-check: a mismatch, or a flag with no corroborating record, refuses and writes nothing. For other harnesses the flag is believed only when the process owning this connection, observed with `lsof` and a `ps` parent chain, descends from the pane's harness process. If the conversation is live as a seat in another process, adopt refuses with `E-RS-CONVERSATION-LIVE` ([double seat](#double-seat-e-rs-conversation-live-plan-174)).
3. **The pane's incumbent**, but only while its recorded `(pid, proc_start)` is alive. A pane id alone is neither identity nor liveness: pane ids and pids both reset at boot. `whoami --pane` applies the same rule, and its refusal names the dead seat.
4. Otherwise a freshly minted name.

`--reclaim <seat>` is an operator override for continuity that cannot heal by itself. It is allowed only when the caller, resolved from its own pane or id, is the target's recorded parent or holds the `prime` role, the harness matches, and the target's process no longer runs. It refuses by name otherwise, before any write. The target keeps its parent and drops its recorded conversation, because that is exactly the evidence that went stale. The status-line heal then records the true one. It emits `seat.reclaimed` with `caller`, old/new proc and pane, `old_harness_session`, `prior_reason` and the evidence.

Claude heals itself. On every render the managed status line checks `whoami --pane`. On a miss (that legacy does not own) or a seat recording a different `session_id`, it fires one detached adopt per pane per 15 s and shows `⛓ registering…` until it lands. When `whoami` is refused with `E-RS-CONVERSATION-LIVE`, or a heal adopt was refused with it, it shows a red `⚠ double: <seat> in %N` instead and fires no further adopt for that conversation. This adds about 1–2 ms to a healthy render. The SessionStart hook retries in the background for about 30 s when the daemon is not up yet. Both write RFC 3339-stamped lines to `~/.pij-rs/hook.log`. The daemon also installs a pij-managed Copilot status line (`~/.pij-rs/copilot-statusline-pij.sh`, wired through `~/.copilot/settings.json`, with the prior script backed up). It is display-only and shows a dead or missing seat as such.

## Native Copilot registration

For a verified native Copilot `/v1/register` claim, `harness_session` normally identifies the durable seat in this machine's registry before pane/process heuristics. Restarting the same conversation reuses its id and adopts the observed host tuple and pane without waiting for the old process or death sweep; tombstone lifting follows the shared continuity contract above. Only live same-session rows compete: two live addresses return HTTP 409 with v2 `error:"refused"` and `details.retryable:true`. Tombstoned rows are history; selection prefers a live row, then greatest `proc_start`, then ascending id as a deterministic final tie. Queued jobs, parent, authoritative `seat_roles` assertion and Telegram reply binding retain that address. `pij state` reports the new process and no tombstone; `seat.native-resumed` records `old_proc`, `new_proc`, `old_pane`, `new_pane` and `reason:"native-session-resume"`.

Explicit verified spawn intent is the exception: a live, unbound Copilot prebind with the exact spawn id and observed pane retains the spawner's allocated id and parent, even when the child resumes a saved conversation. The saved live address is retired with `seat.native-superseded` (`successor`, `spawn_id`, `reason:"native-spawn-prebind"`). Each address keeps its own queued jobs and bindings; nothing is retargeted. The allocated id binds normally, so the `/v1/spawn` waiter observes the child it requested rather than timing out on a discarded prebind.

Same-session replacement relies on Copilot allowing only one live host per conversation through `inuse.<pid>.lock`. The daemon verifies the new host but does not probe incumbent liveness or enforce that external single-host guarantee. If Copilot violates it, the second claimant replaces the recorded tuple while the first may still hold an inbox long-poll: two receivers could temporarily share one address. This is an explicit dependency, not daemon-proven exclusivity.

Different sessions cannot claim an existing session's id; same-pane new conversations retain the new-id/supersede behavior. The cross-machine guard would refuse if foreign provenance were ever recorded, but the SQLite registry clears `machine` on write: rows are local at rest, and that branch currently has only synthetic defence-in-depth coverage. Native clients refresh/retry typed races without inventing an identity; ordinary ownership refusals remain terminal. `/v1/adopt` preserves the same 409 `retryable:true` discriminator if its registration service returns `Retryable`; ordinary adoption does not currently enter the native owner-observation path.

A verified native claim for a session with no existing seat is held when its pane belongs to a live local Copilot seat of another native session: HTTP 409, v2 `command:"pij register"`, `error:"refused"`, `details:{retryable:true,hold:"native-session"}`. The reason names the owning seat and says “awaiting resumed session”. This is not admission: no transient identity, binding or queued work is written. Explicit foreign-address, spawn-correlation and supersedes refusals remain refusals; an existing saved session still uses the normal resume path. The extension stays alive, retries with backoff initially capped at 5 seconds, and shows no timeline line for the first 10 seconds, then at most one “waiting for this pane's resumed Copilot session” notice. After 10 minutes of continuous hold, it emits exactly once: “[pij native] still waiting for this pane's resumed Copilot session after 10 min; pij delivery is unavailable in this window”. The hold retry has no attempt limit or deadline: fork termination is the terminating bound, and after that escalation retries continue at a slower 60-second cadence without exiting. Copilot may stop the throwaway fork when resuming the real session; otherwise, once the old owner is swept dead or its exact incarnation is observed dead, an ordinary retry can admit the genuinely new session.

Same-process supersede is gated by `native_precedence` (plan 168): an optional `{last_user_message_at: ms|null, loaded_at: ms, history_unknown?: true}` on `/v1/register` and on the native receiver heartbeat. When a native claim would retire a live, attested same-process seat of another session, and the claim reports precedence, the claim must outrank the holder. That means a newer `last_user_message_at`, where `null` loses to any value, or an equal one with a later `loaded_at`. Otherwise the daemon answers HTTP 409, v2 `command:"pij register"`, `error:"refused"`, `details:{retryable:true,hold:"native-holder"}`, naming the holder, and writes nothing. The holder is spared only while its receiver lease is live. A lapsed lease, a claim without the field, or a holder whose heartbeat reports no field all keep the earlier newest-registrant supersede. A holder with no report since daemon start keeps the seat until it reports. This check is a **brake**, not a policy: removing it makes registration retire the same seats or more, never different ones. Reports live in daemon memory, on the receiver lease of the reporting incarnation. They are written under the same native lock as the lease decision, and they are monotonic per incarnation, so a delayed or out-of-order heartbeat never lowers them. A committed native registration installs its lease and report inside that lock; a takeover starts the lease afresh, and a repeat registration of the current holder never moves its expiry. An extension that cannot read its history sends `history_unknown: true`, never `null` (observed none) and never an omitted field (old extension). The daemon ranks it as having no prompt: held behind a live holder with history, the later load wins against a blank or unknown holder, and a lapsed lease is taken over. Contract and `/new` behaviour: `docs/how/copilot-native-extension.md` § Several sessions in one Copilot process.

## Codex seats (plan 181)

**What a Codex seat's identity is.** Since Codex 0.157 the TUI can run its
threads in one shared, auto-started app-server daemon (`codex features list`:
`daemon_auto_start`). That daemon keeps the environment of whichever TUI started
it, so every tool shell it hosts can carry another seat's `PIJ_SESSION_ID` and
`TMUX_PANE`. `CODEX_THREAD_ID` is always the shell's own thread. A Codex thread
is named by the writer lock its process holds open,
`<codex home>/thread-writer-locks/<thread>.lock` (`$CODEX_HOME`, else
`~/.codex`), from the moment the thread starts. An in-process TUI holds its own
lock. A daemon-hosted TUI holds none, and its pane's thread cannot be derived.

**Spawn.** `pij-rs spawn --harness codex` launches with
`-c features.daemon_auto_start=false`, so the seat's thread runs in-process and
its shells are children of its TUI. It also passes
`-c check_for_update_on_startup=false`, Codex's own key, so the update menu does
not cover the prompt of a seat nobody is watching. Each of `PIJ_SESSION_ID`,
`PIJ_SPAWN_ID` and `TMUX_PANE` is also set per thread
(`-c shell_environment_policy.set.<NAME>="<value>"`), so a shell sees its own
identity even if a shared daemon hosts it. The launch wrapper appends
`TMUX_PANE` once tmux has named the pane, and records
`<spawn evidence dir>/<spawn id>.child` with its own pid and the Codex child's.

**What binds it.** The daemon's Codex binder, the counterpart of the Claude
binder, binds a pending spawned Codex row when all of these are observed: the
child the wrapper recorded has the wrapper as its parent and holds exactly one
thread lock, the wrapper is the process tmux runs as the pane, and the child's
start is read. The row gets that `proc` and `harness_session` = the thread. A
process outside the pane's tree, or a recorded thread that differs from a
revived seat's conversation, never binds. The binder reads the lock directory
and the roster in-process on every tick and launches nothing unless a Codex
thread started since the last tick or a new pending row appeared. Then it costs
one process-file read per pending row (`/proc` on Linux, one `/usr/sbin/lsof`
on macOS), and one tmux call and one `ps` per bind.

**When it cannot bind.** A Codex spawn that times out adds what is on screen to
`timed out after <n>s waiting for registration:`

- `Codex is showing its update prompt`, quoting its `Update available …` header
  when the capture includes it;
- `Codex is not at its prompt; the screen ends` followed by the last line;
- `Codex is at its prompt, but no registration arrived`.

**How a Codex seat knows who it is.** A caller that sends `CODEX_THREAD_ID`
(`pij` verbs routed through the shim, `whoami`, `adopt --harness codex`) must be
the seat it resolves to: the thread the seat records, or, while it records none,
the thread the Codex process in its pane holds. Otherwise the request is refused
with `E-RS-CODEX-IDENTITY`, naming the thread, the seat or pane it claimed and
what the daemon observed instead, and nothing is written. The refusal
never advises `pij adopt`: adopting the inherited pane is how a Codex seat was
once registered as a parentless stranger. Callers without `CODEX_THREAD_ID` are
unaffected.

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
Both progress fields are required nonnegative safe integers so older extensions
keep parsing, but they are **diagnostics only**: they never decide liveness.
The exact registered session/PID/start tuple remains mandatory.

**Lease liveness means heartbeats arrive, nothing more** (plan 167). Every heartbeat
from the registered incarnation renews the whole lease and answers
`data:{state:"live",lease_ms:60000,renew_after_ms:20000}`
(`0 < renew_after_ms < lease_ms`). A busy host whose turn emits no native event —
one long tool call, a `sleep 120` — is a legitimate wait: the receiver is waiting
for the turn to end and the sender already sees `cold-check: busy`. Plan 167 removed
the earlier frozen-progress brake (K=3 unadvanced renewals → `state:"stale"`) and
the `stale` answer with it. That check was a **policy, not a brake**: removing it
changes a quiet busy turn's outcome from park-and-refuse to wait, rather than making
the operation more conservative. A dead extension still stops heartbeating, so its
lease still expires.

`PIJ_RS_EXT_CLAIM_LEASE_SECS` changes the duration. Register/claim starts the
first lease for an incarnation, but repeated attestation cannot move it; only a
heartbeat renews. If no receiver reconnects after a daemon restart, the expired
boot grace is shown as `native-extension-unavailable`.
A replacement child first attests its identity, then observes a bounded SDK history
window before sending its initial heartbeat or claiming inbox work.

SDK probing is workload-gated: the existing consumption/completion observation loop
reads and checks the independent tail only while a native delivery is pending or its
completion is outstanding. There is no permanent one-second idle SDK timer. An empty
receiver still renews its daemon lease and accepts callbacks, but does not poll the SDK;
the next delivery captures a fresh tail in `newCompletion`. Startup observation remains
a single bounded read. Registration success resets its hold cadence before that read;
a startup observation failure is `receive-held`, never a registration retry episode.
Startup hold is immediately visible in extension stderr; `pij state` exposes the
receiver reason only after the unrenewed lease expires, up to 60 seconds later.

The extension never holds permanently on a lease answer. A `stale` or `unavailable`
answer (an older daemon, or the deploy window), a refused or malformed renewal all
re-register and keep renewing with backoff; local observation counters continue and
nothing is replayed as progress. Genuine integrity holds — malformed claims, an
anchor gap past ten minutes, a native-target hold — still stop receiving and
renewal. Turn-state (`working`/`idle`) publication continues during any hold.

Once a lease expires, `pij state` reports
`native_receiver_reason:"native-extension-unavailable"` separately from host
liveness; human output names the same reason. `pij inbox` and `--peek` include it
in `details.native_receiver_reason` and human-readable `meta`. A live host or an
`idle · active` card alone does not prove that its receiver is renewing.

Expiry parks native bodies with `undelivered:native-receiver-unavailable`,
publishes sender-addressed `delivery.parked`, and refuses subsequent sends.

Pane deliveries (plan 184) add two terminal outcomes, each publishing a
sender-addressed `delivery.parked` with transport `pty`. They cover Claude seats
and Copilot controls on `delivery-control:<seat>`:

- `undelivered:hold-limit`: still held at the hold limit (seven minutes for a
  control command, twenty for a body);
- `undelivered:staged-unsent`: a command whose own staged leftover pij cleared
  and did not submit, or any delivery whose composer did not read blank after
  that clear (`composer not cleared; left for the operator`).

A **body** held past seven minutes stays queued, and its sender is told once
through a `delivery.hold-limit` fact and a `hold-notice-<job_id>` notice,
audited as `delivery.hold-notice`. Schedule, limit and own-text rules:
[pij-pane-signals](pij-pane-signals.md#staged-submits-their-leftovers-and-held-deliveries-plan-184).

**Every park tells its sender once.** For any parked outcome the daemon's
`pij-bg` sends the sender one short message naming the msg id, the target, the
outcome and reason, and "Resend after the target recovers". The notice's msg_id
is derived from the parked job (`park-notice-<job_id>`). It is admitted at most
once across both channels: the queue dedupes the id in any job state, and the
FYI store holds it at most once (`INSERT OR IGNORE`, with an atomic `fyi.held`
fact the follower checks). The notice never wakes a cold seat: it passes the
send guard's own cold-wake `check()`, and where `pij send` would be refused with
`E-RS-COLD-WAKE` it is held as an FYI for the sender's next turn. A refused
delivery (the sender's own receiver is down) also falls back to that FYI.
`delivery.park-notice` (`{msg_id,job_id,notice_msg_id,recipient,channel,notice}`)
is an audit record written after admission and repaired on replay. The follower
subscribes live, then sweeps every live seat's `delivery.parked` facts from the
last 24 hours a page at a time (`Spine::matching_since`, which seeks the
`spine_by_seat_kind_at` index, schema 26), and sweeps again every 60 seconds and
30 seconds after any failed notice. So a park committed before a restart, after
a shutdown abort, or dropped by a lagging subscriber is still notified once. A
sender with no live seat — missing, tombstoned, remote or `pij-bg` — gets no
notice and no error. On first deploy the boot sweep also notifies parks from the
preceding 24 hours.

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
greatest timestamp: a later committed event can have an earlier supplied clock. A seat stalled on its harness's own dialog keeps `state` and `badge`; the roster
and the card add a separate `dialog` object while the stall stands (plan 219,
[Modal-dialog stalls](#modal-dialog-stalls-plan-219)).

`stateNoteAt` (epoch ms) and `stateNoteSeq` (spine cursor) say when the
`report.state` record carrying `stateNote` was written and where it sits, the
way `statusAt`/`statusSeq` cite the card. They come from that record, never from
the card, and are present exactly when `stateNote` is: a declaration without a
note (`done`, `clear`) and a seat that never declared leave all three `null`.
Both keys are always present (plan 188). An empty note cannot occur: every
note path refuses one (`report note must not be empty`). Pinned shape:
[`state-card.json`](../../crates/testkit/fixtures/golden/api/state-card.json).

**Every card read is bounded by the seat's own rows** (plan 188): the card and
the state note are each one newest-of-a-kind lookup on `spine_by_seat_kind`,
and held deliveries read only the seat's `delivery.held`/`released`/`outcome`
rows. None reads the spine end to end, so a card costs the same on a 4M-row
spine as on an empty one.

`deliveries` lists the newest three PRs whose `pij delivered` named this seat
as the PM, each `{project, pr, merge_commit, addresses, recorded_by,
recorded_at}`; `[]` when none (plan 196). Every address of each PR is listed, however many.
Only `POST /v1/state` reads it, in two bounded indexed reads with no sort: the
three PRs by stepping down the covering `deliveries_by_pm` index, then each
PR's addresses through the deliveries primary key. `GET /v1/cards`, which status lines and the PA read often, leaves it `[]`. Human
`pij state` prints one `delivered:` line per PR.

### Every seat's card in one call

`GET /v1/cards` returns `data: {cards: [...], unavailable: [...]}` in a
`pij cards` envelope: one card per **local, untombstoned** seat (the seats
`/v1/seats?scope=local` lists), in seat-id order. Each card is built by the same
function as `POST /v1/state`, so a client can seed every card from one call and
then follow `/v1/events`. Two differences, both so one seat cannot spoil the
fleet's answer:

- **Session status is bounded at 200 ms per seat** (`LIST_STATUS_WAIT`, the
  same bound as `/v1/seats?sizes=true`). A seat whose transcript read runs over
  gets `sessionStatus: {"outcome":"failed","error":"no answer within 200 ms"}`,
  and its size fields follow from that. The read keeps going in the
  background and keeps its cursor, so the next call is warm and complete.
  `/v1/state` still waits for the read.
- **A seat whose card cannot be read is listed in `unavailable`** as
  `{id, error}` (the shape of `/v1/seats`' unavailable peers) instead of
  failing the call. Every other seat's card is still served. Both lists are
  always present; an empty `unavailable` means every seat was read.
 It takes no filters, is local
only (a paired machine's key is refused with `E-RS-PEER-SCOPE`) and adds no
field to `/v1/seats`. It is a new route rather than a flag on the roster
because the roster is polled by federation fan-in and extension rosters that
need none of a card's reads, and a card is a different shape from a row. Cost:
one roster read and one role read, then each seat's indexed card reads and its
session-status fold. Cards are built four at a time, and each card waits at
most 200 ms for its fold. A fold that runs over finishes detached and frees its
slot, so on a cold first call up to every bound seat can be folding at once,
and the response can take up to about ⌈bound seats ÷ 4⌉ × 200 ms (live,
2026-11-02: 1.31 s cold for 88 cards, 4 timed out; 0.21 s and then 0.055 s
warm). `/v1/seats?sizes=true` already folds every seat at once. No process launch beyond
`/v1/state`'s own (an in-process liveness probe per bound seat). The fold reads
the whole transcript cold and only what was appended warm, through the
session source's existing per-seat cursor cache. One bulk call warms that
cache for every bound seat.

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
consent, and transport errors. `leftover-cleared` (plan 184) means pij just
cleared its own staged leftover and the next attempt stages into a blank
composer. Not counted here: native Copilot inbox-pull holds
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
- `coldWake`: `{wouldRefuse, estimateUsd?, notEstimable?}`. This is `cold_wake::check` itself, so it is exactly what the guard would do with a normal send now. A HydraFusion router seat (plan 169 R7, bound model `hydrafusion`/`hydrafusion-max`) refuses on the same size and idle rules but never carries `estimateUsd`: `notEstimable` reads `not estimable: router, per-call models`, because the router picks the model per call and no single model's price describes the wake. Its refusal and `pij state` line show the price as `? (not estimable: router, per-call models)`.

`sizeLines` holds the rendered human lines, which every client prints verbatim:
- `context 720k / 1M · last call 2h ago · cache 1h (cold 1h) · 3 compactions`;
- when the guard would refuse, `❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi only if it has nothing to do; wakes no one)`.

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

An FYI is for information the recipient does not have to act on ([the FYI rule](../../skills/pij/references/00-routing.md#c10--wire-discipline-a2a-messages), rule 3); anything they have to do something about, "step done, your turn" and hand-offs included, is a normal send even when no reply is needed. It never opens a turn: the daemon holds it until the recipient's next real turn, and it arrives with that turn, so an idle recipient will not see it until something else wakes it. The only definition of the appended block is the golden fixture [`fyi/block.txt`](../../crates/testkit/fixtures/golden/fyi/block.txt); clients pass `block` through byte-for-byte and never re-render it.

```bash
pij send <seat> --fyi 'text'           # prints: pij send: held (fyi), <seat> was NOT woken — <msg_id>
pij-rs send --to <seat> --fyi 'text'   # native form
```

| Surface | Contract |
|---|---|
| POST `/v1/send` with `"fyi": true` | An ordinary send body plus `fyi`. The daemon stores the FYI durably and opens no turn: no tmux, UDS or extension push. `data` is a Receipt whose `outcome` is `{"outcome":"held","reason":"fyi"}`. Refused (`ok:false`, `error:"refused"`) with a control `command` or a remote `seat@machine` recipient; a tombstoned or unknown recipient gets the same errors as a normal send. |
| Receipt `data.warning` (plans 159, F169) | Present only on a held FYI. A body containing `?`: `this looks like a question; if you need an answer, resend without --fyi`. Otherwise a body that reads as actionable (`done`, `complete`, `ready`, `your turn`, `over to you`, `handing off`, `blocked`, `please`, `can you`, `review`, `verdict`, `PR #` and the like, matched at the start of a word): `this reads as actionable: send it normally so it wakes them`. The FYI is still held; nothing is converted or refused. The shim prints it on the line after `pij send: held (fyi), <seat> was NOT woken — <msg_id>`; the OMP/Pi `pij_send` tool appends it on its own line; the Copilot `pij_send` tool returns it as a top-level `warning` beside `receipt`. |
| Warm flush (plan 159) | When the hold that brings a seat's pending FYIs to `FLUSH_AT` (5) finds the seat's prompt cache known **warm**, the daemon delivers them now as one queued message from `pij-bg`, whose whole body is the block beginning `5 FYIs were queued for you:`. Warm needs positive evidence: the last API call is known and within the seat's real cache lifetime, `min(cache TTL, 60 min)`. A `working` state is not evidence, and an unknown TTL, an unknown last call, or no answer within 3 s means hold. The FYIs are claimed in the carrier's own transaction, exactly once (`fyi.delivered` names `message:fyi-flush-<seat>-<ms>`). If a hook or ride-along claimed them first, nothing is queued. A turn boundary (`POST /v1/activity`) never flushes: a seat starting a turn gets the pile with that turn. A failed flush leaves the FYIs pending and never fails the hold. |
| Digest (plan 159) | A block for more than `DIGEST_ABOVE` (5) FYIs, whether ride-along, hook claim or flush, is a digest. It has a header with the total and a count per sender, the newest `DIGEST_NEWEST` (3) in full, numbered by their place in the pile, and a last line naming the read command: `All N were delivered; read them in full with: pij-rs fyi-read --seat <seat> --claimed-at <ms>`. Every FYI is marked delivered. The golden fixture is [`fyi/digest.txt`](../../crates/testkit/fixtures/golden/fyi/digest.txt). |
| POST `/v1/fyi/read` · `pij-rs fyi-read --seat <S> --claimed-at <ms>` · `pij-rs fyi-read --seat <S> --pending` | Read-only. Request `{seat, claimed_at_ms}` or `{seat, pending: true}` (exactly one). `data` is `{seat, count, block}`, where `block` is every FYI that claim delivered, in full, oldest first, headed `N FYIs were delivered to you:`; with `pending`, every FYI still held for the seat, headed `N FYIs are held for <seat>, not yet seen:` and claiming nothing (F169: what a PA reads before deciding to wake the seat). Empty when none match. The CLI prints the block. |
| Ride-along | The next non-control, non-FYI message to the seat carries every pending FYI, appended to the same body as `<original body>\n\n<block>` and never sent as a separate message. The FYIs are claimed by the same transaction that creates the message's durable queue row (`fyi.delivered` names `message:<msg_id>`), so an FYI is never claimed without its carrier. A retry or duplicate of a message the queue or ledger already has claims nothing. A pane-bound direct send (socket or typed) that would carry FYIs is queued instead, and the drain worker delivers it, after any job already running on its serial key. Its receipt is `queued` with reason `queued as a normal message (it wakes the seat when delivered); carries N held FYIs`: the message is never an FYI, whatever a role request's `--reply`, and only `--fyi` holds. The reason used to be `fyi-ride-along`, which read as a downgrade (F116, F120). Observed history, not a promise: in the two days to 2026-11-08, all 381 such sends on the live fleet were delivered by injection within 5 s. A paneless socket seat's direct sends carry none, so its FYIs arrive through the typed-turn hook. A failed FYI claim never fails the send: the send goes out plain and the FYIs stay pending. No client work is needed. |
| POST `/v1/fyi/claim` | Typed-turn hooks only. Request `{seat?, pane?, native_session?, via}`, where `via` is `hook:claude`, `hook:copilot`, `hook:omp` or `hook:pi`. At least one of `pane` or `native_session` is binding evidence; `seat` may be omitted when `pane` resolves a live seat, and a `seat` that mismatches the pane or session refuses. `data` is `{seat, count, block, ids}`; `count: 0` means `block: ""`. The claim is atomic: pending → delivered exactly once, with one `fyi.delivered` event carrying `via`. Racing claims, or a claim racing a ride-along, never deliver the same FYI twice. |
| `pij-rs fyi-claim --pane <P> [--native-session <S>] --via hook:claude` | Prints the raw `block` to stdout, or nothing at count 0, and exits 0. Global `--json` prints the envelope instead. An unreachable daemon exits non-zero; the calling hook then prints nothing and exits 0, so it never blocks the human's prompt. |
| `POST /v1/state` → `data.pendingFyis` | Pending count on the camelCase card (`pij state <seat> --json`). |
| Stall notice (F169) | An FYI wakes no one, so a hand-off sent as one can stall its recipient. The PA watchdog's round (no loop of its own) finds each seat holding two or more FYIs from peers (`pij-bg`'s own notices are not counted), observed idle, whose oldest has waited 15 minutes. Its scope's PA gets one normal message from `pij-bg` (the scope's prime when there is no live PA; a cold PA or prime is woken, audited with reason `pa-watchdog: seat may be stalled on held FYIs`), naming each FYI's sender, age and fenced first line, under 2 KB, with `pij-rs fyi-read --seat <seat> --pending` and the normal send that would wake it. The seat itself is never woken. Claimed in `presence_claims` as `fyi-stall` before it is sent, so it goes once per backlog: the claim re-arms when the backlog is gone or a newer FYI arrives. |
| PA line (F169) | Every turn handed to a seat holding the `pa` role (a message, a nudge, a warm flush; not a control, not a hook-claimed typed turn) ends with one line naming the seats in its fleet with waiting peer FYIs, oldest first, at most 5, then `+N more`: `Waiting FYIs: pij-x 3 (oldest 22m, idle), pij-y 1 (5m, working)`. The fleet is the repository of the PA's parent (else its own), read from the files with no launch; the PA's own pile is not listed, since it rides along on that turn. Omitted when no seat is waiting, and never stored: a queued turn gets it when it is handed over (an inbox, extension or native claim, or the pane worker typing it), so it is as of that moment; a direct socket or typed delivery gets it at admission. The pane worker keeps a job's noted text for its retries in this run, so a retry types the same text and pij still knows its own leftover. At most 300 bytes, so a stall notice with the line appended stays within 2 KB. Computed from the store: a role lookup by seat on each delivered turn; for a PA, one grouped read of pending FYIs, then a registry read only when some are waiting. |
| Held state notices (F170) | A daemon notice held as an FYI is re-checked when it flushes (ride-along, warm flush or hook claim) and whenever it is read (`fyi-read`, a digest's full read or `--pending`; the stored body never changes), against each seat's registry state and its last size from the PA watchdog's round. A large-seat line whose seat is now at or under 600k reads `(since resolved: now 67k)`; an idle notice whose seat is working again starts `(since resolved: now working)`; a closed seat reads `(since closed)`. Nothing is dropped. Only `pij-bg`'s own `large-seats:` and `idle-nudge-fyi:` notices are touched. |
| `/v1/whoami` → `data.pending_fyis` | The same count on whoami (`pij-rs --json whoami --pane "$TMUX_PANE"`). Status lines and footers read it here, with no extra call, and show `✉N` only while N > 0. |
| An FYI id already taken (review of #189, S189-001) | An FYI's identity is `(origin machine, msg_id)` (migration 0027). The same sender's retry to the same seat is the same FYI: held once, and the retry also answers `held`. An id already held, from the same origin, for **another sender or recipient** is refused: nothing is held, the hold fails with `E-RS-FYI-ID-TAKEN` (a send failure, `ok:false`), and `fyi.refused` is appended on the intended recipient with `id`, `sender`, `held_recipient`, `held_sender` and `reason: "id-taken"`. It used to answer `held` and drop the FYI silently. |
| `/v1/events` kinds `fyi.held`, `fyi.delivered` | `seat` is the recipient. Extensions refresh their count on any `fyi.*` event, or `seat.tombstone`, for their seat. |
| `seat.tombstone` payload `pending_fyis_dropped` | A tombstone drops the seat's pending FYIs and records how many in this field, present only when at least one was dropped. |
| Hook claim window | A hook claim that commits in the daemon and then times out in the client (the hooks give up after 3 s) delivers nothing: those FYIs are claimed at most once, never twice. This is the one remaining loss window. |

A claimed `block` is placed as: Claude `UserPromptSubmit` hook `additionalContext`; OMP and Pi `before_agent_start` custom message `pij-fyi`; Copilot native extension `onUserPromptSubmitted` `additionalContext`.

Source: [`fyi.rs`](../../crates/core/src/fyi.rs).

### Busy/idle publication: POST `/v1/activity`

The request is `{seat, pane?, native_session?, state: "working"|"idle", context_tokens?, context_window_tokens?, context_token?}`. It takes the same binding evidence as `/v1/fyi/claim`, and a tombstoned seat is refused. `context_tokens` (plan 217) is the session's current size when the runtime knows one the transcript does not: it must come with `context_token`, the grant the seat's own native registration returned (`/v1/register` answers it, random, one per process incarnation; never in a roster or on the spine). A report without it, or with another seat's or an earlier incarnation's, is refused and changes nothing, the activity included: the pane and session above are public, so they cannot authorize a size that moves the cold-wake guard (review S153-001). Known limit (F88, PRD 003): registration does not authenticate the calling process, so a local caller re-registering with a live seat's roster identifiers is handed its standing grant; each such reissue appends `context.grant.reissued` `{seat, claimed_id, harness, folder, pane, pid, proc_start, session, attempt, binding, proc_source, origin}`, never the token, and if that append fails the grant is withheld (the registration stands). A first grant for an incarnation writes nothing. Genuine re-registrations write the event too, so it is an audit trail, not a theft detector. A native registration and the applying of a size report run in one critical section, and a report is checked again inside it against the seat's current process and grant, so a report from a replaced incarnation changes nothing (review S153-002). The daemon keeps the latest in memory, per seat and session, and fills an unknown `contextUsedTokens` (and `contextWindowTokens`) from it with basis `native` ([PA watchdog § presence](#pa-watchdog)).
It changes only the seat's mechanical `state` (a targeted write that never rewrites or revives the row), so `pij state` shows `working` during a turn and `idle` after it. Each change publishes a `seat.activity` event. `data` is `{seat, state, changed}`.
- `working` never outlives its process: a tombstone resets it to `idle`, and so does a new incarnation registering the seat.

- The OMP/Pi extension publishes `working` at `turn_start`, and `idle` at `turn_end`, at `agent_end` and on shutdown.
- The Copilot native extension publishes on `assistant.turn_start`, and on `assistant.turn_end`, `session.idle` or shutdown. Each post carries the latest `session.usage_info` (`currentTokens`, `tokenLimit`) as `context_tokens` and `context_window_tokens`: that event is ephemeral and never reaches `events.jsonl`. It sends them only with the grant its registration returned; a refused report (an older daemon, or a grant from before a daemon restart) makes it resend the state alone and wait for its next registration's grant.
- Publications are serialized per session, and a failure never breaks a turn. A route-absent 404 from an older daemon stops publishing.
- Claude seats publish through their managed hooks (plan 157): `UserPromptSubmit` sends `working`, and `Stop` and `StopFailure` send `idle`. `StopFailure` covers a turn that ends on an API error, which does not fire `Stop`. The seat is resolved by pane, via `pij-rs activity`, with the same 3 s bound and silent failure as the FYI claim. Only a prompt submitted to Claude fires `UserPromptSubmit`, so a turn opened another way stays `idle` until the next typed or pasted prompt.
- The cases these hooks cannot see leave a Claude seat reading `working`:
  - **An Esc interrupt** fires no Claude hook: neither `Stop`, `StopFailure`, nor the `idle_prompt` notification (verified live). The seat reads `working` until its next prompt or completed turn. The cold-wake guard repairs the costly case. Three conditions must all hold: the seat is cold by its facts alone, its `working` fact is itself older than the idle threshold, and one probe of its live pane (1 s bound, off the request thread) shows **positive** idle evidence, `HarnessPort::idle`. For Claude, idle evidence means a ready footer, the composer row between its rules, and no spinner row. Then the send is refused, and the seat is corrected to `idle` with `seat.activity` reason `stale working (esc)`. Everything else allows and leaves `working` alone: a younger `working`, no pane, a failed or slow probe, or any other frame (empty, blank, truncated, a permission dialog, a tool run). The `working`-age condition exists because a Claude streaming a reply shows no spinner and looks idle (live 2.1.284 frames).
  - **A killed or exited process** fires nothing. The death sweep's tombstone, or a new incarnation registering the seat, resets it to `idle`.
  - While a seat reads `working`, the cold-wake guard treats it as busy and allows the send. That is the safe direction for a brake.

### Mid-turn steering (plan 214, ruling C31)

A normal send lands inside a turn the recipient is already in, wherever the harness can take it there: OMP and Pi steer every message into a running turn, Claude's socket transport delivers between tool calls, and the native Copilot extension steers too. Until C31 the Copilot extension claimed its next pij message only after the turn its previous pij message started had ended (the post-ack barrier), which held messages for hours behind long turns (F25). Now, while such a turn runs, it keeps claiming, and each claimed normal send goes in with `send` mode `immediate`, which steers it into the running turn. The turn is still observed and reported (`native-completed`) when it ends. FYIs are unchanged: held until the recipient's next real turn.

**The switch.** `pij steer on|off|status [<seat>]` (`POST /v1/steer`, argv + `caller`) sets a seat's mid-turn steering; on by default, and the seat defaults to the caller. Any seat may switch any live seat, as with the watchdog. A real change appends a `steer.changed` event, and a seat changed by another is told in a held FYI. `status` lists `{default: "on", off: [{seat, set_by, set_at_ms}]}`, with `{seat, steer}` when a seat is named. The setting lives in `seat_steering` (migration 0043); no row means on.

**What off means, exactly.** Off restores the old post-ack barrier and nothing more: after a pij message starts a turn, the extension claims the next one only when that turn has ended. A first pij message into a turn the seat started itself (a human's turn) still goes in at once, as it always did. Turning steering back on takes effect at the next turn boundary at the latest.

**How the receiver reads it.** Every native claim (`GET /v1/inbox`) reports the seat's setting as `details.mid_turn_steer`; the extension keeps the last value and treats a missing one as off, so an older daemon keeps the barrier. During a turn its own message started, the extension claims only when the last value was on, and marks the claim `during_turn=true`. The daemon reads the setting again at the hand-off, after the job is claimed and before it is returned, so a claim that was already waiting when `off` was set never steers a message: the job is released and the answer is the hold `native-consumer-held:turn-wait: …`, and the extension waits for the turn to end before its next claim. A change therefore takes effect at the seat's next hand-off, with no restart and no poll.

**Receipt note.** A normal send that is queued for a working seat that may not take it before its turn ends gets a receipt `warning`: `<seat> is mid-turn and <why>: this message may wait for that turn to end.` The reasons are steering off (worded as applying when a pij message started the turn), a Copilot seat with no native extension, a Claude seat with no inbound socket, and Codex. OMP, Pi, socket Claude and native Copilot with steering on get no note.

A message still held long behind a busy turn shows up in the PA digest (`Messages held behind a busy turn`, below), which is how a wedge or a seat with steering off is noticed.

Cost: no new launch. One primary-key read per native claim (the switch) and one more per during-turn hand-off, a registry read per queued send (the note), and one more claim per message during a turn instead of one after it. A during-turn claim's wait is event-driven, not the native 500 ms tick: it wakes on an enqueue to the seat or a `steer.changed` event for it, with a 15 s backstop (`DURING_TURN_BACKSTOP`) for work that becomes claimable on a timer, so a busy seat with an empty inbox costs at most one wake per 15 s. The extension observes a running turn with one shared watcher per turn, however many messages are steered into it.

### Cold-wake guard (plan 157 phase 2)

Waking a seat whose prompt cache has expired re-writes its whole context at cache-write prices. The daemon therefore refuses a waking send to a **cold** recipient. The constants, the price table and the pure rule live in [`cold_wake.rs`](../../crates/core/src/cold_wake.rs): context over `COLD_CONTEXT_TOKENS`, last API call older than `COLD_IDLE_MS`, and the daemon does not see the seat working. The guard is a brake: it only ever refuses, and it never changes what is delivered.

**Copilot seats** (plan 217). Copilot's transcript carries no context size, so until plan 217 the guard read Copilot seats as `unknown: context size unknown` and never refused them. Copilot seats now report their size at each turn boundary ([`/v1/activity`](#busyidle-publication-post-v1activity)), so once a Copilot seat has reported its size, a normal send to it while it is idle, cold and over the threshold is refused with `E-RS-COLD-WAKE`, as it already is for other harnesses: the sender resends with `--fyi`, or with `--force --reason '<why>'`. The threshold and idle time are unchanged. A Copilot seat whose size is not yet known (none reported since the daemon started) is still never refused. The price shown is the same Anthropic list-price estimate: it does not model Copilot billing, which charges AI credits per token type and writes a 5-minute cache, not a 1-hour one (a known gap, not a rule).

```bash
pij send <seat> --force --reason '<why>' 'text'             # shim → /v1/shim/send argv verbatim
pij-rs send --to <seat> --force --reason '<why>' --body 'text'   # native form
```

| Surface | Contract |
|---|---|
| Refusal | HTTP 400, `{ok:false, error:"refused", meta:…}`. Nothing is sent. Since plan 160 the meta lists the sender's options, each priced (golden [`cold-wake/refusal.txt`](../../crates/testkit/fixtures/golden/cold-wake/refusal.txt)):<br>`E-RS-COLD-WAKE: ❄ pij-x is cold (720k, idle 1h52m). Options:`<br>`  --fyi                  only if they need do nothing; wakes no one  $0 now`<br>`  --force --reason "…"   wake on its current model                   ~$6.45`<br>`Estimates at list price: a 1-hour cache write of the whole context, plus about 5 calls.`<br>The wake estimate is `cold_wake::wake_estimate_usd`. The first call writes the whole context to the 1-hour cache. Each of the 4 further calls reads the context so far, writes ~1,585 new tokens and outputs ~590, which are the 2026-09-28 usage study's means. Every call outputs ~590. Prices come from the `PRICES_USD_PER_MTOK` table (Opus 5.5, Sonnet 5.5, Haiku 4.5, list prices checked 2026-10-01). Model ids are normalised first, and an unpriced model says `price unknown`. The shim prints the meta alone on stderr; the OMP/Pi and Copilot `pij_send` tools return it verbatim as the tool error. |
| POST `/v1/send` `"force": true, "reason": "<why>"` | Wakes a cold recipient anyway. `force` without a non-empty `reason` is refused with a meta starting `E-RS-COLD-WAKE:` that names `--reason`. The tools refuse it too, before any request. |
| Spine `send.cold-wake-forced` | The audit of every forced cold wake, persisted before the wake. `seat` is the recipient; payload `{from, from_machine, msg_id, reason, context_tokens, idle_ms, model, estimate_usd, not_estimable}`; `from_machine` is null for a local sender and the paired machine's alias for a forwarded one. |
| Receipt `data.cold_check` | `clear`, `busy`, `forced` or `unknown: <why>` on every guarded send; absent when the guard did not run. The shim prints `— cold-check: <verdict>`; the OMP/Pi tool appends `(cold-check: <verdict>)`. |
| Unknown ⇒ allow | Any fact the rule needs but cannot establish (unbound or unsupported harness, no transcript, a source error, unknown context or last-call time) allows the send as `unknown: <why>`. The guard waits 3 s for the recipient's session facts; after that the send is allowed as `unknown: no answer within 3s`. |
| Never guarded | FYIs (`fyi:true` opens no turn) and controls (`command`). |
| One recipient per send | rs has no multi-recipient send, and the shim refuses broadcasts, so the per-recipient rule is met by one recipient per send. |
| Remote peers (plan 164) | A `seat@machine` send is guarded by the **receiving** daemon, exactly as a local send. The sender's daemon forwards `force`/`reason`/`fyi`, waits up to `federation_first_attempt_wait_secs` (10 s) for the first forwarding attempt, and answers with the receiver's receipt or refusal: a cold refusal reads `E-RS-COLD-WAKE: …` with `details:{code, seat, cold:{verdict, contextTokens, idleMs, model, estimateUsd}, machine}`. A peer that does not answer in time leaves the row queued (`Queued` receipt); the outcome later lands as `delivery.outcome` or `delivery.remote-refused` (payload `{msg_id, peer, reason, details?}`) on the sender's spine. A forced remote wake is audited on the receiver with `from_machine` naming the sending machine. A message's identity on the receiver is `(from_machine, msg_id)`: queue dedupe, the delivered ledger and the FYI table each keep the origin in its own column (migrations 0027, 0028), so a peer's id never collides with a local one. No msg_id may contain `@` (`E-RS-ARG`). Every rendering shows a forwarded sender as `seat@alias`. |
| `pij bounce` announcement | The bounce announcement is an ordinary send from `pij-daemon`. A cold seat refuses it, so the bounce output lists `announcement failed … E-RS-COLD-WAKE` for each cold seat, and those seats are not woken. |
| Cursor warm-up | At daemon start every live, bound seat's session is read once in the background, one at a time with a pause between seats. The first send after a bounce is then a warm read. A read the guard stops waiting for still completes and keeps its cursor. |

Sources: [`cold_wake.rs`](../../crates/core/src/cold_wake.rs) (rule), [`http/cold_wake.rs`](../../crates/daemon/src/http/cold_wake.rs) (guard and audit).

## End-of-turn blocks (plan 226)

A seat can end its reply with a fenced `pij` block of fire-and-forget commands instead of making one tool call per send. Its harness hands the final reply to the daemon when the turn ends, so the closing sends cost no extra model calls.

````markdown
Merged and bounced; handing over.

```pij
send pij-steady-otter "MERGED #210, daemon bounced"
send role:reviewer --reply wake "review #211 at abc1234"
output verdict --role reviewer --subject pr:211 --head abc1234 --result pass
```
````

**What runs.** Only a block that ends the reply: the reply's last fenced block, opened by a line that is `` ```pij `` (indentation allowed; a longer backtick fence works too), closed by a bare fence at least as long, with nothing but whitespace after it. Fences are tracked from the start of the reply as Markdown tracks them, so a `` ```pij `` line inside another open fence (a ` ```text ` block, or a four-backtick fence quoting a whole example) is that fence's content and never runs. A block quoted mid-reply, followed by prose, or never closed runs nothing. One command per line, written like the CLI without the leading `pij` (a leading `pij` is tolerated); blank lines and `#` lines are skipped. Quoting is shell-style on one line and expands nothing: `'…'` is literal, `"…"` honours `\"`, `\\`, `\$` and `` \` ``, a backslash outside quotes escapes the next character. The verbs are `send`, `output` and `requests close`; anything else is invalid, and nothing is ever run by a shell. At most `MAX_COMMANDS` (20) commands.

**How it runs.** Every command is validated first, by its verb's own parser (`/v1/shim/send`'s, `/v1/output`'s, `/v1/requests close`'s), plus three block rules: `--help`, a send's `--body-file` and an output's `--body-file -` are invalid, because nothing at turn end can read them. A send also meets the send path's own rules that read nothing but the line, the same functions the send path calls later: `--force` needs a non-empty `--reason` (on every non-FYI send, a role send that would only queue included), `--reply`, `--limit` and `--project` belong to a send to `role:<id>`, a role address must fit `role:<id>[/<instance>]` and is never sent to another machine, a role send's `--reply` and `--limit` values must read, and a `--file` path must resolve. If any command is invalid, nothing runs. Then the commands run in order, as the calling seat, through those same route handlers, so the caller ladder, the cold-wake guard, FYI holds, role resolution and every refusal are exactly the CLI's. The run stops at the first refusal; a refusal is an `ok:false` envelope or a receipt whose outcome is `refused`. The run is its own task, so a harness that stops waiting cannot cut a block off between two commands.

| Surface | Contract |
|---|---|
| POST `/v1/turn-end` | Request `{text, caller, via?}`, unknown fields refused: `text` is the whole final reply, `caller` the `CallerContext` the shim and governance routes take (send `cwd`: an output's `--body-file <path>` resolves against it), `via` (`hook:claude`, `hook:copilot`, `hook:omp`, `hook:pi`) is informational. Local key only; not peer-reachable. |
| Answer | `ok:true` whenever the text was processed; `data` is `{outcome, lines, ran, summary, followup}`. `outcome` is `none` (no trailing block; `lines` empty, `summary` and `followup` null), `ran`, `invalid` (nothing ran) or `refused` (stopped at a refusal). Each `lines` row is `{line, command, verb, status, detail, code, result}`: `line` counts commands from 1, `status` is `ran`, `refused`, `invalid` or `not_run`, `detail` is one line (the receipt, or the reason), `code` the refusal's `E-RS-…` when it has one, `result` the verb route's own `data`. |
| `summary` | One line for the human: `pij ✓ 2 sent, 1 output`, `pij ✗ nothing ran: line 2 invalid: "…"` or `pij ✗ line 2 refused: "…" (1 ran, 1 not run)`. The reason is fenced as caller text: its first line, escaped, in quotes, cut at 120 characters. |
| `followup` | Null unless the block failed. Otherwise the text of the seat's one follow-up turn, naming what ran (`Ran, do not resend:`), what was refused or invalid and why, and what did not run, so a retry never sends twice. Each command and reason in it is fenced caller text (escaped onto one line, in quotes). |
| A bad request | HTTP 400 `E-RS-ARG`, naming the contract. |

**Harness rule.** The daemon never sees a stop hook's state. A harness calls the endpoint only when the reply may end with a block (its trimmed text ends with `` ``` `` and some line is `` ```pij ``), shows `summary`, and when `followup` is non-null gives the seat one follow-up turn with it, only if its own stop hook is not already active, so a failure never loops. OMP: `session_stop`, which fires once at the terminal settle with the final text-only reply (never for a tool call, an abort, a retry or a subagent); the line is a `pij-turn-end` custom message posted once the session is idle (a notice raised during the settle never draws); the block is hidden on screen by an assistant-text renderer, which needs the OMP setting `extensionRenderers.assistantText` (`just link` sets it; `just omp-doctor` reports it off, with the fix). If the daemon gives no answer, the seat is told some commands may have run and to check before resending; that reason is cut to its first line and 120 characters, control characters escaped. The renderer costs one substring check per draw for a reply without `` ```pij ``, and remembers its last 32 answers for replies with one.

Cost: one request per turn end whose reply ends with a `pij` block; nothing otherwise; nothing per tick. The daemon launches nothing for it: each command launches what its verb launches from the CLI (an output's writer commit). Known limit: a harness that retries the request after the run has finished runs the block again, because each send mints its own message id; the harnesses call once per stop.

Sources: [`turn_end.rs`](../../crates/core/src/turn_end.rs) (parse, judge, render), [`http/turn_end.rs`](../../crates/daemon/src/http/turn_end.rs) (validate and run), [OMP adapter](../../.omp/extensions/pij/adapters/omp-turn-end.ts).

## Send to a role (plan 199)

`pij send role:<id>` (or `role:<id>/<instance>` for a many-holder role) is resolved by the project's community directory, never by `seat_roles` (ruling C1). The project is `--project <slug>`, else the one whose repo holds the caller's cwd. The role document is found in the governance worktree without launching git. Live holders are open holdings whose seat is registered and not tombstoned.

| Live holders | What happens |
|---|---|
| one | An ordinary send (or `--fyi` hold) to that seat, through the cold-wake guard. The body gains one header line, `[pij role:<id>[/<instance>] request <id> from <seat> · reply <mode>[ · limit <d>]]`. The request is recorded `delivered` with its holder. |
| none | The request is recorded `queued` (an `--fyi` send too, C2), and the project's `pa` holder, else the current prime designation, else a live `prime`-role seat, is woken once through the ordinary delivery path (a cold wake is audited, as the PA watchdog does). The wake names the request and two runnable steps: revive the last holder (with its context size) or `pij directory assign`. pij never starts a seat. Queued requests are listed in the PA watchdog digest until drained, and so is any request stuck `delivering` (claimed for a holder and never settled), with its holder and age. The digest reads both through partial indexes and never reads bodies, and only for the PA's projects (plan 209): those it holds a role in, else those its prime holds a role in, else every project when its parent is the designated prime, else none. |
| two or more | Refused `E-RS-ROLE-HOLDERS`, naming each seat with its instance (or `no instance`). When a holder has an instance, `next` addresses it (`pij send role:<id>/<instance> "<text>"`); when none has, an instance cannot pick one, so `next` is the fix, `pij directory assign <id>/<instance> <seat> --why "<why>" --project <slug>`, and the message names a seat send (plan 212, F52). |

An instance address resolves against the role's open holdings, so an instance a PM assigned on a one-holder role (`reviewer/<pr>`) is addressable. On a one-holder role whose holdings have no such instance it refuses `E-RS-ROLE-ADDRESS`, with `next` `pij send role:<id> "<text>"` when one seat holds it, or a seat send when several do, so the two refusals never point at each other.

The receipt is the ordinary send receipt plus `request: {id, project, role, instance, reply, limit_ms, state, holder}`. The request id is the send's `msg_id` (C3); a queued receipt has `outcome: queued` and a `woke: {seat, outcome, cold_check}` object. Assigning a holder (`pij directory assign`) delivers that role's queued requests to it in order, each from its requester, and marks them delivered (C6); a refused assign drains nothing. The assign waits for its drain, and its receipt's `drained` lists each request it settled, `{request, from, reply, outcome}`, where `outcome` is `delivered`, `closed` (with `why`) or `queued` (with the `error` that left it there); `pij directory assign` prints one line per entry (plan 212, F56). A request the new holder sent itself is closed, not delivered (a seat cannot send to itself, and it knows what it asked): `why` is `the requester took the role` (F61). A request for any holder drains to a holder of any instance; one for an instance drains only to that instance. Each request is claimed before it is delivered, in one store statement that also requires the seat to be live and to hold the role at that moment, so two drains never deliver one request and a drain that runs after its holder was released delivers nothing (the request stays queued). A drained non-FYI request is admitted like an immediate send: when the holder is cold, a `send.cold-wake-forced` audit naming the requester and the reason `role queue: drained to the role's new holder on assign` is written before the wake, and a failed audit sends nothing. A drain never refuses a cold holder: the assign is the deliberate act, and a refusal would strand the request until the next assign. A daemon that stops between a claim and its outcome leaves the request `delivering`. The next boot settles every such row before it serves anything: one the delivery path already took for its holder (ledger, delivery job or held FYI under its msg_id) is marked delivered, and any other goes back to `queued` with no holder, so the next assign drains it once and the PA digest lists it. Waking the requester when the holder answers (C4) is s06's; s05 only stores the reply mode.

`pij requests [--role <id>] [--state queued|delivering|delivered|closed] [--project <slug>]` lists the project's requests, newest first, at most 100 per state, with no bodies; with no `--state`, the open ones (queued and delivering). `pij requests withdraw <id> --why <text>` closes a queued request: in one transaction it moves to `closed_role_requests` (migration 0040) with who, when and why, so no assign delivers it and the PA digest drops it. The requester, a holder of the project's `pa` role or the project's recorded prime (`projects.prime_id`, as `pij output --answers` reads it; a `prime` seat role or designation a caller gives itself grants nothing) may withdraw; anyone else is refused `E-RS-OWNERSHIP`. A retried send of a closed request's msg_id replays it as `closed` and never queues it again (plan 212, F59).

`pij requests close <id> [--why <text>] [--quiet]` closes a delivered request that was answered or abandoned (F83). It is the same one-transaction move, keeping its `holder` and `delivered_at`, and it stops the request's time limit: no further check-in or escalation. A request answered with a plain `--in-reply-to` send is the case this exists for. Withdraw's authority applies, plus the request's holder. `why` defaults to `answered or abandoned`. Unless the requester closed it or `--quiet` is given, the requester gets one FYI (msg id `request-closed-<id>`), reported as `told` in the receipt. The FYI is sent by the daemon (`pij-bg`), never as the closer (ruling C32 (1)), and its body names the closer, how that caller was bound (`bound by pane` or `asserted seat id`) and the reason. The close records what it owes (`notice`: `quiet`, `owed`, then `told`) in the same transaction; the FYI is sent by a task of its own, so a cancelled caller never abandons it, and a failure is in the printed line. Closing an already-closed request whose notice is still `owed` sends it then, replaying the close as it was recorded and audited (its closer, binding and reason; the retrying caller needs only the authority to trigger it, and its own `--why` is not used); a repeat after that is refused `E-RS-REQUEST-CLOSED`. How the caller was identified matters for `withdraw` and `close` (security review of #149, S003, F88): a caller bound by its pane (the daemon checks the pane, and an asserted id that disagrees with it is refused) has the full authority above, while a caller known only by an asserted session id acts only as the request's requester or holder, never as its PA or prime. Every withdraw and close writes one `request.closed_by_actor` spine event, appended in the close's own transaction: request, project, verb, the claimed actor, how it was bound, pane and session where given, and `quiet`; no body. Known limit (ruling C32): a local caller holding the daemon key can assert the requester's or holder's id and close or withdraw that request, which suppresses its clock; the close is audited, and the requester's notice names the closer as an asserted seat id. Nothing is ever sent as that seat. PRD 003 req-10 binds callers properly. A queued or delivering request is refused `E-RS-REQUEST-NOT-DELIVERED`; for a queued one, `next` is the withdraw.

Flags, on a send to a role only: `--reply wake|fyi|none` (default `wake`; stored for s06 to notify with) and `--limit <duration>` (`90s`, `45m`, `1h30m`; default the role's `default_limit`, C5; stored for s12). `--project <slug>`. All three ride `POST /v1/send` as top-level `reply`, `limit` and `project` beside the message. The shim hands a role send to `pij-rs` unchanged ([shim forwarding](#shim-forwarding-plan-213)), so its receipt is pij-rs's: `from <seat>; request <id> <state> to <holder> (reply <mode>)`.

| Refusal | When |
|---|---|
| `E-RS-ROLE-ONLY` | `--reply`, `--limit` or `--project` on a send to a seat |
| `E-RS-ROLE-ADDRESS` | a malformed address, an instance on a one-holder role, `role:<id>@<machine>` or a forwarded role send (role addresses are local and never forwarded), or `--command` to a role |
| `E-RS-DIRECTORY-ROLE-MISSING` | the project's directory has no such role (next: `pij directory view`) |
| `E-RS-DIRECTORY-RETIRED` | the role is retired (plan 206); nothing is sent or queued. A merged role names the role it went to, and the next step is `Send to role:<to> instead.` |
| `E-RS-ROLE-HOLDERS` | two or more live holders (next: an instance address only one live holder has, with `--project`; a seat send when an instance is shared; or the assign that gives a holding one) |
| `E-RS-REQUEST-NOT-QUEUED` | `requests withdraw` of a request a holder already has (delivering or delivered) |
| `E-RS-REQUEST-CLOSED` | `requests withdraw` of a request already closed |
| `E-RS-NOT-FOUND` | `requests withdraw` of an unknown id |
| `E-RS-OWNERSHIP` | `requests withdraw` by a seat that is not the requester, the PA or the prime |
| `E-RS-ROLE-MISMATCH` | the holder's `seat_roles` role disagrees with the directory (a `pm`, `pa` or `worker` role other than the one the directory implies; a prime is never re-roled). Next: `pij role <seat> <role>` |

Cost: a role send costs what an ordinary send costs, plus in-process reads (no launch) and one store write; a queued request adds one wake delivery; a drain one delivery per request, or one store write for a request its new holder sent. `pij requests` is one indexed store read per state (`role_requests_by_project`, `closed_role_requests_by_project`); `withdraw` is one write transaction. Nothing is launched. No timer or poll: the digest reads the queue once per nudge.

### Time limits on role requests (plan 205)

A role request with a limit (`--limit`, else the role's `default_limit`, C5; with neither, none) is checked inside the existing PA watchdog round, with no timer of its own. The clock starts when the request reaches the holder's session (plan 214, F73): its delivery ledger row, else its acknowledged delivery job, else its delivered FYI, one indexed read. While delivery still holds it behind a busy turn there is no check-in, however late; time spent queued for a holder, or held behind the holder's turn, does not count. After that the clock runs from the later of arrival and the holder's latest `report.now` card, so every card update restarts it. A check-in for a request delivery held a minute or more says how long (`It waited 20m in delivery before it reached you; the limit counts from then.`). A holder that is no longer live while its request is still held gets no check-in; the PA is told when the limit passes, as below. Past the limit, the holder gets one check-in through the ordinary delivery path, with a cold wake audited as the watchdog does: write down what you have, update your card, and say why it needs longer. If a whole further limit passes after the check-in with no card update, the project's `pa` holder (else the prime) is told once. A holder that is no longer live cannot be checked in, so the PA is told as soon as the limit passes, with that reason. The notice says how the holder went (observed dead, closed, or no such seat) and, when the seat can be revived, names `pij-rs revive <seat>` for its parent or the prime to run (F161). A round sends one message per recipient, holder and kind: several requests due together are listed in one check-in or notice, oldest first, capped at 2 KB with `...and N more`, under the oldest request's msg_id, and each is stamped when that message is admitted (F161). Nothing is ever stopped. Each step is first claimed (`limit_claim_at`, migration 0035) in one statement that also requires the request to be unanswered (`answered_at`, set by s06's `pij output --answers` or by a reply: a local send from the request's holder with `--in-reply-to <request id>`, plan 214, F68, to the holder's requester or to a role, when that send is accepted; a reply from any other seat, or one its recipient refused, answers nothing, and the first answer's time is kept, so neither path counts twice. Under ruling C32 a reply's caller is bound where its context resolves (a `from` that differs is refused `E-RS-REPLY-SENDER`, and context that does not resolve is refused as the identity ladder refuses it); a reply to a role request, open or closed, must be bound by the caller's pane, or, for a paneless native-extension seat (a Copilot seat outside tmux replying through its `pij_send`), by the native session it registered with (`HARNESS_SESSION_ID` matching the seat's, audited as `native session (registered)`; F154). A caller with no context, or with only an asserted session id (`PIJ_SESSION_ID` with no pane, which the ladder checks for existence only), is refused `E-RS-REPLY-IDENTITY`, normal or FYI, to a seat or a role, so a reply is never delivered as the seat it names (C32 (1)). That native session is roster metadata any client can read, so it binds at registration strength only, which ruling C33 accepts (the trust boundary is the user account); a paneless seat that is not a native-extension seat, or a caller giving another session, still never binds. `pij-rs send`, the OMP/Pi `pij_send` tool and the Copilot `pij_send` tool attach the caller's context to every reply. Both tools take an `in_reply_to` parameter naming the request answered (OMP's defaults to the last message from the recipient; F154). The answer and its `request.answered_by_reply` audit event commit in one transaction, so there is no answer without its audit. Known limit: UNCORRELATED sends still accept an asserted session id (existence only) and a raw `from`, and paneless pull seats with no native extension (Claude or Codex outside tmux) cannot send correlated replies; they send without `--in-reply-to` and answer with `pij output --answers`. Pane binding is registration strength (C32 (2)): a client holding the daemon key can supply a live seat's pane on a correlated reply and send as that seat, and the same send-as is reachable today through any uncorrelated send with a `from`. All of these end with PRD 003 req-10 (plan 221 phase 2, which gates send-as first; C32 (4))) and not yet told at that step, so an answer that lands mid-round wins and no two rounds send at once. The step's stamp (`checkin_at`, `escalated_at`) is written only once delivery admits the message (delivered, queued or held). When delivery refuses it, or there is nobody to tell, the claim is dropped and the next round tries again. Before sending, the attempt pins the seat it resolved (`limit_claim_target`). A claim left by a daemon that stopped mid-attempt expires after two minutes, and the retry first asks delivery whether the pinned seat already has the step's message (`role-limit-checkin-<id>` or `role-limit-notice-<id>`: a ledger row, delivery job or held FYI, as boot recovery reads). If it does, the retry only stamps; it never resolves a new recipient, so a PA that rotated meanwhile is not told twice. If it does not, the attempt never reached admission, and the retry resolves the recipient afresh.

Cost per round: one indexed read of the open limited requests (delivered, limited, not escalated, not answered), plus one `LIMIT 1` card read per open request; a check-in or notice is one claim, one pin, one delivery and one stamp; a retry of a stopped daemon's claim adds up to two indexed admission probes. A notice with nobody to tell is retried each round, at one claim, one assignments read and one release per such request. No launch site and no timer.

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
| POST `/v1/steer` | `steer on [<seat>]`; `off [<seat>]`; `status [<seat>]`; argv + `caller` only | — ([Mid-turn steering](#mid-turn-steering-plan-214-ruling-c31)) |
| POST `/v1/watchdog` | `watchdog on [<seat>] [--every <duration>]`; `off [<seat>]`; `status [<seat>]`; argv + `caller` only | — ([Opt-in watchdog](#opt-in-watchdog)) |
| POST `/v1/report` | `report now/state/question/blocked/clear/verify`; state metadata flags below | `report-question`, `report-verify` |
| GET and POST `/v1/anomalies` | `anomalies [--here] [--project <slug>]`; GET `here=<absolute-path>&project=<slug>` | `anomalies-list`, `anomalies-argv` |
| GET and POST `/v1/decisions` | `decisions [--state <open/answered/all>] [--asked_by <seat>] [--parent <seat>]`; GET keys `state`, `asked_by`, `parent` (literal underscore; `--asked-by` refuses) | `decisions-list`, `decisions-argv` |
| POST `/v1/answer` | `answer [--supersede] <decision> <answer>`; typed `{decision, answer, supersede?, caller}` | `decision-answer`, `decision-answer-supersede` |
| POST `/v1/close` | `close <seat> [--reason <text>]`; typed `{seat, reason?, caller}`. A role holder's close answers `held` and lands after its [memory turn](#the-memory-turn) | `seat-close` |
| POST `/v1/reap` | `reap [--dry-run]`; typed `{dry_run?, caller}` | `reap-dry-run` |
| POST `/v1/spine` | `spine append --kind <kind> [--refs <a,b>] [--project <slug>]`; `events/render [--since N] [--peer <seat>] [--project <slug>] [--limit N]` (paged, see below) | `spine-append`, `spine-events`, `spine-render` |
| POST `/v1/directory` | `directory [--project <slug>]` (the view); `directory add <id> --why <text> [--project <slug>]` with role flags or `--file <json>`; `directory add --batch <dir> --why <text>`; `directory add <id> --from template:<name>[@v<n>]\|file:<brief> --why <text>`; `directory edit <id> --why <text>` with `--set <field>=<value>`, role flags or `--file <json>`; `directory retire <role> --why <text> [--drop-queued]`; `directory merge <from> --into <to> --why <text> [--drop-queued]`; `directory templates [--show <name>[@vN]]`; `directory template-diff <id>`; `directory assign\|release <role>[/<instance>] <seat> --why <text>`; `directory assign --batch <file> --why <text>`; `directory release --batch <file> --why <text>`; `directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text>`; `directory lineage add --batch <file> --why <text>`; `directory external add <seat> --from-project <slug> --why <note> [--role <id>]`; `directory rebuild`; `directory memory add <role> --title <t> --hook <h> [--tags <a,b>] --body <text>\|--body-file <path>`; `directory memory <role> [--note <slug>]`; `directory routes [--write --why <text>]`; `directory [<verb>] --help`. See [Community directory](#community-directory-plan-190), [Routing table](#routing-table-plan-202) and [Role memory](#role-memory-plan-201) | — (`crates/daemon/tests/directory_http.rs`) |
| POST `/v1/output` | `output verdict\|deliverable\|note\|handover --role <id> --subject <address> [--head <sha>] [--result <r>] [--note-owed <text>]… [--evidence <json>]… [--link <url>] [--answers <msg_id>] [--body <text>\|--body-file <path>] [--project <slug>]`; `output --help`. See [Role outputs](#role-outputs-plan-200) | — (`crates/daemon/tests/output_http.rs`) |
| POST `/v1/outputs` | `outputs --subject <address> [--head <sha>]`; `outputs --role <id> [--limit <n>]`; `outputs --help` | — (`crates/daemon/tests/output_http.rs`) |
| POST `/v1/claim` | `claim <kind> --for <subject> [--project <slug>]`; `claim release <kind> <number> [--why <text>] [--project <slug>]`; `--help`. See [Number claims and delivery at merge](#number-claims-and-delivery-at-merge-plan-196) | — (`crates/daemon/tests/claims_http.rs`) |
| POST `/v1/claims` | `claims [--project <slug>] [--open]` | — (`crates/daemon/tests/claims_http.rs`) |
| POST `/v1/delivered` | `delivered <pr> [--pm <seat>] [--project <slug>]` | — (`crates/daemon/tests/claims_http.rs`) |
| POST `/v1/requests` | `requests [--role <id>] [--state queued\|delivering\|delivered\|closed] [--project <slug>]`; `requests withdraw <id> --why <text>`; `--help`. See [Send to a role](#send-to-a-role-plan-199) | — (`crates/daemon/tests/directory_http.rs`) |
| POST `/v1/adopt` | Existing identity admission plus optional asserted role; omitted parent and role remain unsaid | `adopt-with-role` |
| POST `/v1/register` | Existing verified registration plus optional asserted role; failed admission cannot commit a role | `register-with-role` |

The case-id column names exact `.routes[].cases[].id` entries in [`governance-routes.json`](../../crates/testkit/fixtures/golden/api/governance-routes.json): select the row's method/path, then that id for its pinned `request` and `response` (and `shim_request` when supplied). For combined GET/POST rows, `*-list` is GET and `*-argv` is POST. A GET fixture's `request:null` means no request body. The existing report now/state event payload examples are separately pinned by `report-now` / `report-state` in [`governance-events.json`](../../crates/testkit/fixtures/golden/api/governance-events.json); the new report route cases are question/verify. These pointers are fixture provenance, not new captures.

`--json` is accepted by the command surfaces. Read families share their GET and POST parser/projection; GET `here=true` cannot guess the caller's folder and refuses. Exact peer/project filters do not widen scope. `status-stale` is node-keyed, so a project filter can omit it: supervisors query anomalies unscoped before declaring cards fresh.

**Bounded reads (issue #119).** `spine events` and `spine render` read one page: at most `--limit` rows after `--since` (default 500, maximum 5000; `0`, a larger value or a non-number refuses `E-RS-ARG`). `--peer` pages that seat's rows on its own index; `--project` filters the page, so a page can return fewer rows than it read. `cursor` is the last row read when the page was full, and `"more": true` is then present: call again with `--since <cursor>`. Otherwise `more` is absent and `cursor` is the spine head, exactly as a complete read always returned. `node show`'s `cursor` is the spine head. `pij anomalies` reads per seat through the `(seat, kind)` indexes (cards, done and verify claims, the newest `seat.put`, delivered dispatches, and the ack/clear events of each row's seat), so its cost follows the registry, not the spine's length; it used to fold every spine row on every call (4.18M rows, ~7.5 s and a ~6.5 GB daemon peak).

**Seat roles (plan 166).** Every setter enforces the closed vocabulary `prime | pm | worker | pa` and refuses anything else (`E-RS-ARG`, naming the allowed list). This covers `role`, `orchestration role set`, `register`/`adopt --role`, spawn and link. Roles are stamped by a governor, never inferred or backfilled. The placement verbs accept only `pm | worker | pa`; `prime` comes from designation.

**Seat ids (F127).** A new seat's id takes a closed grammar: `pij-`, then words of lowercase ASCII letters and digits joined by single hyphens, at most 64 bytes (`SeatId::is_canonical`). Every generated name and every live id fits it. Anything else is refused as `E-RS-SEAT-ID` (`details.code`; the reason shows the id escaped, never raw): by `register` wherever the claim would mint a seat (a claim that lands on an existing row or a spawn's prebind keeps that row's id), and by `spawn --id`. `adopt` never takes a caller's id. A refused role's reason is escaped the same way.

- `POST /v1/spawn` with `role` resolves `caller`. That seat becomes the parent and `assigned_by`; a different `parent` refuses `E-RS-OWNERSHIP` before launch. The seat row, `seat.put`, the `seat_roles` row and `role-set` commit in one transaction.
- `POST /v1/link` lets the caller take a live seat whose recorded parent is absent or not live, or re-role a seat it already parents. A parent change appends `seat.put`, a role change appends `role-set`, both in one transaction, and an unchanged role appends nothing.
- Link refusals:
  - `E-RS-OWNERSHIP` with `details.parent` for a live foreign parent;
  - `E-RS-OWNERSHIP` with `details.reason:"prime"` for a prime;
  - `E-RS-ARG` with `details.reason:"cycle"` for the caller's own ancestor.
- The receipt carries `seat`, `parent`, `previous_parent`, `role`, `assigned_by`, `assigned_at`, `parent_changed`, `role_changed` and `seqs`.
- A registration that claims no `--parent` keeps the parent the row holds at commit (`put_reporting_keeping_parent`). A refresh racing a link therefore never reverts the placement.
- Self-asserted `adopt --role` is unchanged, a known divergence from TS. Placement needs the SQLite registry: fake-registry daemons refuse it rather than desync.

Project/stream/fence/dispatch/task records are rs store authority, not files under `~/.pij/`. Fences describe intended writes, not permission. Stream close changes its record, not the worktree. Dispatch persists packet digest and outbound linkage before delivery; queued is not delivered, and delivered is not acknowledged. Only the resolved recipient can ack the matching packet SHA; identical ack is idempotent. Canary requires actual nonce-correlated dispatch/ack and observed runtime/model evidence, not descriptor presence. `attest --plan-id` never grants native-extension-delivery attestation. `node show` joins rs records only. `spine render` returns `{text,cursor}` and never writes a legacy ledger file.

Source: [`governance.rs` parser and handlers](../../crates/daemon/src/http/governance.rs), [`report.rs`](../../crates/daemon/src/http/report.rs), [`anomalies.rs`](../../crates/daemon/src/http/anomalies.rs), [`decisions.rs`](../../crates/daemon/src/http/decisions.rs), [`lifecycle.rs`](../../crates/daemon/src/http/lifecycle.rs).

## Community directory (plan 190)

`pij directory add` writes one role definition, a `pij/role` document, to
`<government-root>/community/roles/<id>/role.dd.json` on the `prime-governance`
branch, with `memory/INDEX.md` and `outputs/` beside it. `<government-root>` is
the standing governance worktree's `.harness/government`, found as
`harness/scripts/government-root.sh` finds it. The daemon is the only writer:

1. It refuses before writing if any path it is about to write carries an edit it
   did not make (modified, staged or untracked), if that path or a directory
   above it inside the worktree is a symlink (checked with lstat, before anything
   is written and again just before each write), or if the role's `role.dd.json`
   already exists. The check covers only paths the write will actually write.
2. It writes the document and has ddocs validate (`--depth 0`) and build it, by
   the absolute path resolved at daemon start.
3. It commits in the standing governance worktree with `git add -- <its paths>`
   then `git commit --only -- <its paths>`. Everything else staged, unstaged or
   untracked there stays as it was, and the checkout's own next commit sees
   pij's files as committed. No plumbing moves the branch under a checkout, and
   pij never pushes.
4. It records `(project, path, commit)` in the store (migration 0031). The rows
   can be rebuilt from the branch head.

While it stages and commits, the writer keeps `pij-writer.owner` in the
worktree's own git dir (beside its `index.lock`, found by reading the
worktree's `.git` file, no launch) naming the daemon pid, the time and the
commit subject, and removes it when the write ends, however it ends (plan 208,
F18). A person whose own commit in the governance worktree fails on
`index.lock` and finds that file should retry in a moment. The writer's own
`git add` and `git commit` wait on a foreign `index.lock` for about 1.5 s
(100, 200, 400, 800 ms), then refuse `E-RS-GOV-LOCKED`; nothing else retries.

Writes are serialised per governance worktree. The branch also carries copies of
pij's `pij/*` schemas under `.dd/schemas/pij/`, so ddocs validates there; a write
refreshes them in the same commit when they differ from pij's.

Flags: `--title`, `--kind specialist|general`, `--lifecycle long-lived|per-task`,
`--holders one|many`, `--instance-key`, `--default-limit` (like `45m`),
`--summary`, `--template`, `--template-version`, `--harness`, `--model`,
`--effort`, and the standing packet as `--packet <text>` or `--brief <path>` (a
repo-relative link to an existing file). ddocs has no union type, so the writer
checks the pairing itself: an inline packet carries `text` and no `brief`, a
brief packet `brief` and no `text`, else `E-RS-ARG`. ddocs only warns on a
missing link target, so the writer also checks, before writing anything, that a
brief is a readable file inside the governance worktree (not absolute, no `..`,
no symlink out), else `E-RS-ROLE-BRIEF-MISSING`. These repeat: `--rule`, `--scope-path`,
`--out-of-scope`, `--forbidden`, `--input`, `--output-kind`. A field you leave
out is left out, so ddocs names it (a role without scope or a standing packet
refuses). `--file <json>` gives the whole document instead of flags. `--batch
<dir>` imports every `*.dd.json` directly in `<dir>` in one commit, each naming
its own `meta.id`; one invalid document commits none. `--why` is required and
becomes the changelog row (`date`, `by` the calling seat, `why`). The project is
`--project`, else the project whose recorded repo holds the caller's directory,
else the one whose repo shares the caller's common git directory, so a linked
worktree finds its project (read from git's files, no launch).

A role folder that exists without `role.dd.json` (a hand-written role with its
own `memory/`, `outputs/` and `role.md`) is adopted: the write adds
`role.dd.json` and its `.dd.md` and touches nothing else there. A new role
also gets `memory/INDEX.md` and `outputs/`; an existing folder keeps its own.

The commit message is `community: add role <id>` (or `community: import <n>
roles`), then the `--why`, then `Pij-Seat` and `Pij-Project` trailers. The
envelope returns `data.commit`, `data.paths` (repo-relative), `data.roles` as
`[{id, path}]`, for a single role `data.id` and `data.path`, and `data.routes`
(see [Routing table](#routing-table-plan-202)).

| Refusal | When | Next step in `details.next` |
|---|---|---|
| `E-RS-GOV-ABSENT` | no standing `prime-governance` worktree, or it is detached, missing, another repo's, or has no `.harness/government` | the bootstrap step `government-root.sh` prints |
| `E-RS-DDOCS-MISSING` | ddocs was not found at daemon start, or will not run | install ddocs or set `PIJ_DDOCS`, then restart the daemon |
| `E-RS-DDOCS-INVALID` | ddocs rejected a document; the message is ddocs' own, prefixed with the path | — |
| `E-RS-GOV-FOREIGN-EDIT` | a path the write needs carries an edit pij did not make | commit or remove those edits |
| `E-RS-GOV-LOCKED` | the worktree's `index.lock` stayed held through about 1.5 s of retries; pij never removes it | let that git command finish |
| `E-RS-GOV-SYMLINK` | a path the write needs (the role, its `.dd.md`, a schema copy) is, or sits under, a symlink inside the governance worktree; a clean tracked link is no foreign edit to git, but writing through it could land outside the worktree | replace the link with a real file or directory |
| `E-RS-DIRECTORY-EXISTS` | the role's `role.dd.json` already exists; names that file, and a batch names every such role | choose another id |
| `E-RS-NOT-FOUND` | no project records the caller's repository (or a named project or seat does not exist) | `pij project create <name> --repo <main checkout> --prime <seat>`, or `--project <slug>` |
| `E-RS-ROLE-BRIEF-MISSING` | a standing packet's `brief` is not a readable file inside the governance worktree; names the role and the brief | commit the brief first, or use `--packet` |
| `E-RS-GOV-GIT` | another git failure | — |
| `E-RS-ARG` | a bad id (1-48 lowercase letters, digits or hyphens), a missing `--why`, or mixed `--file`, `--batch` and flags | — |

Every refusal writes nothing, commits nothing and leaves the worktree as it was.

Cost: three git launches to find the worktree, then one `status`, one `add`, one
`commit` and one `rev-parse` per write, plus a ddocs `validate` and `build` per
document (each a node process). Launches are counted as `git.community` and
`ddocs.community`. Nothing runs on a timer.

Source: [`community/`](../../crates/daemon/src/community/), [`http/directory.rs`](../../crates/daemon/src/http/directory.rs).

### Holding roles (plan 195)

`pij directory assign <role>[/<instance>] <seat> --why <text>` binds a live seat
to an existing role. It appends a row to
`community/roles/<id>/holders.dd.json` (`pij/holders`: seat, harness, harness
session, machine, instance key, from, to, assigned by, released by, why, release
why) through the same writer, then indexes it in the store (migration 0032) and,
in that same store transaction, sets the seat's role (ruling C1): `pa` for the
role `pa`, `pm` for `pm` or `pm-<n>`, `worker` otherwise; a prime is never
re-roled. Then it calls the after-assign hook (ruling C6), which s05 uses to
drain the role queue. A second live holder of a one-holder role succeeds with
`data.warning` `W-ONE-HOLDER`. The envelope carries `commit`, `path`, the `row`
as written, `seat_role` and `flushed` (pending close-ends this write carried to the branch;
only those lose their pending mark).

An instance key a holding is recorded with (`assign`, single or `--batch`, and
`lineage add`, single or `--batch`) takes the one name grammar every
daemon-authored message renders by (F135, `pij_core::render::is_name`):
`[a-z0-9][a-z0-9-]{0,31}`. A `role:<id>/<instance>` address takes the same.
Anything else refuses (`E-RS-ARG`, or `E-RS-ROLE-ADDRESS` for an address),
showing the key escaped and bounded (32 characters), or the whole address
escaped and bounded (64 characters) for an address refusal. `release` and `rebuild` still read a stored
key as it is, so a holding recorded before the check can be ended. Such a stored key never reaches a window name raw: the
window name (`<project>:<role>[-<instance>]`, in tmux and in `data.window.name`)
shows each part only through its grammar, the kept-window reason no longer
repeats the name, and the plain receipt renders the name whole.

`pij directory assign --batch <file> --why <text>` (plan 208, F16) reads one
`<role>[/<instance>] <seat>` per line (blank and `#` lines skipped) and records
every holding in one writer commit (`community: assign <n> holdings`), all or
nothing: an unknown role, a seat that does not exist or is closed, a holding
already open, or a line repeated in the file refuses the whole batch, naming
every bad line, and writes nothing. Each holding then gets what a single assign
gives it (store row, seat role, hook, window rule). The envelope carries
`commit` and `holdings`, one receipt per line in file order.

`pij directory release <role>[/<instance>] <seat> --why <text>` closes the row
(`to`, `released_by`, `release_why`); seat roles are left as they are.
`pij directory release --batch <file> --why <text>` (plan 215, F71) reads the
same `<role>[/<instance>] <seat>` lines and ends every holding in one writer
commit (`community: release <n> holdings`), all or nothing for the directory
write: the holders documents either all change in that commit or none do. A git
or store-index failure partway through is the writer's inherited split (the
commit landed, the store did not index it; `pij directory rebuild` restores
it), not a global recovery. Its diagnostics are
phase-local: a malformed line refuses `E-RS-ARG` naming every malformed line;
only a file that parses is checked against the holders documents, where a
holding that is not open, a line repeated, or a missing role refuses
`E-RS-DIRECTORY-NOT-HELD` (or `E-RS-DIRECTORY-ROLE-MISSING`) naming every one.
Either way nothing is written. Each live seat's window then follows its
remaining holdings as for a single release, checked once per seat against its
earliest ended holding. The envelope carries `commit` and `holdings`, one
receipt per line in file order. Through the shim, assign, release and both
batches print one line per holding (`released <role>[/<instance>] from <seat>
(commit <sha>)`, plus the window); `--json` keeps the receipt.

`pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time>
--why <text>` (plan 208, F15) lets the prime record a closed seat's past
holding, for history kept elsewhere before the directory existed. The seat need
not be live or even registered. Only the project's prime (its `prime_id`; for a
project that names none, a seat whose role is `prime`) may run it
(`E-RS-OWNERSHIP`). Times are RFC 3339
with `Z` or an offset, fractions allowed, or a bare date (UTC midnight), stored
as UTC to the millisecond; `--from` must be before `--to` and `--to` not in the
future (`E-RS-ARG`). A holding that overlaps another of the same seat and
instance refuses `E-RS-DIRECTORY-OVERLAP`. The row goes into the holders
document in time order, with `released_by` the prime and `release_why`
`lineage add`, in one writer commit; the store indexes it without a live-seat
check and sets no seat role. The view shows it in the lineage.
`pij directory lineage add --batch <file> --why <text>` (plan 209, F67) reads
one `<role>[/<instance>] <seat> <from> <to>` per line (blank and `#` lines
skipped) and records them all in one writer commit (`community: lineage <n>
holdings`), all or nothing: a bad time, a missing role, or an overlap with a
committed holding or an earlier line refuses the whole batch and writes
nothing. Its diagnostics are phase-local (review of #144, N001): a line that
does not parse (a bad or reversed time) refuses `E-RS-ARG` naming every such
line, and only a file that parses is checked for missing roles and overlaps,
which are then named together. The envelope carries `commit` and `holdings`.
Through the shim, `lineage add` (single or `--batch`) prints one line per
recorded holding, `recorded <role>[/<instance>] held by <seat> from <from> to
<to> (commit <sha>)` (plan 215, F66); `--json` keeps the receipt.

`pij close <seat>` ends every open holding of the seat with `release_why`
`close`, and never fails because of the directory. The ends are stored as
pending first; each project's commit clears them. If the writer refuses (no
governance worktree, a foreign edit), the close still tombstones, its receipt
names the pending ends under `data.directory.pending` with the refusal, and the
next directory write for that project (any verb, including an assign or release
of the same role) carries them to the branch.
Committed ends are under `data.directory.committed`. A seat with no holdings
adds nothing to the receipt.

`pij directory` (no verb) shows this project's roles (id, title, kind, holders
mode), their live holders (seat, instance, since, machine) and the full lineage,
plus pending close-ends; `--json` gives the data. It reads role documents from
the governance worktree and holders from the store, in-process, and launches
nothing. `pij directory rebuild` replaces the project's pointer and holder rows
from the governance branch head, so the view after a store loss is
byte-identical (pending ends that never reached the branch are dropped: the
branch is the truth). `pij directory [<verb>] --help` prints the verbs and
flags, with no daemon needed.

`pij directory external add <seat> --from-project <slug> --why <note> [--role
<id>]` appends a dated entry (date, seat, from_project, role, note, by the
calling seat) to `community/external.dd.json`, created with `meta.project` on
first use (req-16). Loans are not otherwise tracked.

Every date the writer stamps carries its zone (plan 208, F13): holders `from`
and `to`, and since plan 208 changelog `date` rows too, are UTC to the
millisecond with `Z` (`2026-11-06T00:07:12.123Z`); older changelog rows keep
their bare UTC date. The human `pij directory` view prints each holder's
`since` in local time with its offset (`2026-11-06 10:07:12 +10:00`, reading
the offset once per view); `--json` keeps the stored UTC value.

Holders rows are written so `to` and `released_by` always come together (ddocs
cannot pair them; the writer refuses otherwise with `E-RS-DIRECTORY-ROW`).
Release and close keep the assign's `why` and write their reason to
`release_why`. Times are UTC to the millisecond (`2026-11-04T05:00:00.000Z`).
A holding is its seat, instance and `from`, so the writer gives each new holding
a `from` no earlier row of that seat and instance has (an assign, release and
reassign in one second makes two rows). A rebuild refuses a holders document
that records one holding twice with `E-RS-DIRECTORY-ROW`, and replaces nothing.

| Refusal | When |
|---|---|
| `E-RS-DIRECTORY-ROLE-MISSING` | the role has no `role.dd.json`; the next step is `pij directory add` |
| `E-RS-DIRECTORY-HELD` | the seat already holds that role and instance |
| `E-RS-DIRECTORY-PRIME-DESIGNATED` | `assign` (single or `--batch`) of the `prime` role: the prime is designated, never assigned (C24); the next step is `pij project set <project> --prime <seat>` (F117) |
| `E-RS-DIRECTORY-NOT-HELD` | release of a role the seat does not hold |
| `E-RS-NOT-FOUND` | the seat does not exist |
| `E-RS-DIRECTORY-OVERLAP` | `lineage add` of a holding that overlaps another of that seat and instance |
| `E-RS-OWNERSHIP` | `lineage add` by anyone but the prime |
| `E-RS-ARG` | a closed seat, a missing `--why`, role flags on assign or release, a bad `--batch` line (all named, nothing written), or a bad, reversed or future `lineage add` time |

On assign the holder's tmux window is renamed `<project>:<role>`, or
`<project>:<role>-<instance>` for a many-holder role (req-26, C16), when the new
holding is the seat's first open one (the earliest `from`). A seat that takes a
second role keeps its window's name (plan 208, F17): no launch, and
`data.window` says `renamed: false` with the holding that names it. Releasing the
holding a live seat's window is named for renames it for the seat's next first
open holding, or back to the seat's own name (what spawn names a window) when it
holds nothing else (plan 212, F57); releasing any other holding leaves the
window alone, with no launch. A rename is one tmux
launch (`tmux.rename-window`) that targets the holder's pane id, never through
the control client, whose allow-list stays reads only. pij never targets a
window by that name: tmux reads `-t a:b` as session `a`, window `b`, so a name
like `pij:security` is ambiguous as a target. A seat with no pane, or a failed rename, still
assigns; `data.window` says what happened.

Cost: assign costs one writer commit plus the rename (about 10 launches: git 7,
ddocs 2, tmux 1; 9 when the rename is skipped); release, lineage add and a close
that ends holdings cost one writer commit (about 9), and a release that renames
the window one more tmux launch. `assign --batch` of N
holdings is one writer commit (git 7, ddocs 2 per holders document touched)
plus at most one rename per line; `release --batch` is one writer commit plus
at most one rename per seat. Each writer commit also reads the role documents
in-process to check `ROUTES.md` (no launch). The view and help launch nothing. A rebuild
costs three git launches plus one per holders document, on demand only.

### Role templates (plan 194)

pij ships versioned role templates (`templates/roles/<name>/v<n>/role.dd.json`:
prime, pm, pa, coder, reviewer and security), compiled into the daemon with
`include_bytes!` like the schema copies; a test fails if a file there is not
compiled in. They support Pij Emergent Communities (PEC, ruling C26): the
prime and the humans build whatever community the work needs now and reshape it
as the work changes, and the templates are examples, not a checklist (C18).

`pij directory templates` lists each template with its versions, the latest
version's title, kind and summary, and whether it is a core role
(`data.templates`). `pij directory templates --show <name>[@vN]` returns
`data.template`: `{name, version, latest, path, body}`, where `body` is that
version's exact embedded bytes (the latest by default) and `path` its place in
pij's repo (`templates/roles/<name>/v<n>/role.dd.json`). An unknown name or
version refuses with `E-RS-ARG`, listing what exists. Besides the core set and
security, ten optional templates ship as examples (devcontainer, researcher,
note-taker, topic-expert, docs-keeper, infra, release, qa, designer,
transcriber; plan 204).

`pij directory add <id> --from template:<name>[@v<n>] --why <text>` copies the
named version (the latest by default) through the writer above. `meta.id`
becomes `<id>`, `meta.template` and `meta.template_version` record the template
and version, and the template's own changelog is dropped so the role's first row
is the add's. Role flags given alongside override the template's fields: a
single flag replaces that field, a repeated flag replaces the whole list, and
`--packet` or `--brief` replaces the standing packet. `--template` and
`--template-version` cannot be combined with `--from template:`.
`--from file:<brief>` is the spelling of `--brief <brief>` (the brief is
referenced, not copied); giving both refuses. `--from` takes no `--file` or
`--batch`. An unknown template or version refuses with `E-RS-ARG`, naming it and
listing the templates or versions that exist, and writes nothing.

`pij directory` gives each role `template`: `{name, version, latest, behind}`, or
null for a role from no template (`latest` is null for a template this build
does not ship). A role whose template has moved on (`behind: true`) is marked in
the human view with its `template-diff` line. `pij directory template-diff <id>`
returns `{id, template, version, latest, behind, diff}`: `diff` is a unified diff
of the template between the role's version and the latest, computed in-process,
and empty when the role is current or came from no template. Nothing is ever
applied: the role file is untouched. A role without `role.dd.json` refuses with
`E-RS-DIRECTORY-ROLE-MISSING`. A role that records a template but no
`template_version` refuses with `E-RS-DIRECTORY-NO-TEMPLATE-VERSION`, and a
version that is not a number with `E-RS-ARG`; both name the next step, `pij
directory edit <id> --set template_version=<n> --why <text>` (plan 206, F19).

The view also carries `prompts` and `guidance` while any core role (prime, pm,
pa, coder, reviewer) or security is missing (ruling C22). A role counts for a
template when its id or its `meta.template` is that name, so `pm-001` made from
`pm` clears the pm prompt. Each prompt is `{role, kind, command}`, `kind` core or
recommended, and `command` a runnable line such as `pij directory add pm --from
template:pm --why 'core role from the pm template' --project fleet`. Every
printed command names the viewed project, so running it from another repo's
directory still acts on the directory that printed it. Each word of a printed
command is shell-quoted where it needs it (`picture::command`), including text
from step documents, blueprints and the store and placeholders such as
`'<seat>'`, so the line runs as written: nothing in it is substituted or
redirected (security S001 on #141). A test splits every printed command with
`/bin/sh` and replays it through the parser its verb reaches;
the human view's drift hint does the same. `guidance` states the PEC wording. The view creates nothing; adding a role clears its prompt.

Cost: listing, diffing and the view read embedded bytes and the governance
worktree in-process and launch nothing; an add from a template costs the same
writer commit as any add. No loop or timer.

### Reshaping roles (plan 206)

`pij directory edit <id> --why <text>` changes a role that exists (F20). Give
`--set <field>=<value>` (repeatable; `title`, `kind`, `lifecycle`, `holders`,
`instance_key`, `default_limit`, `summary`, `template`, `template_version`,
`harness`, `model`, `effort`, `packet` or `brief`) and any of the role flags
`add` takes, or `--file <json>` for the whole document. The writer reads the
committed role under its lock, applies the change, keeps the changelog (a
`--file` document's own changelog is replaced by the committed one), adds one
row with `--why`, and commits it like an add (validate, build, commit, pointer),
with `ROUTES.md` regenerated in the same commit when the role's scope changed
(plan 202).

A list flag (`--scope-path`, `--rule`, `--out-of-scope`, `--forbidden`,
`--input`, `--output-kind`) replaces the whole list, as for `add --from`. To
change one item, each list has a pair: `--add-<list> <item>` and
`--remove-<list> <item>` (for example `--add-rule`, `--remove-scope-path`),
repeatable and `edit` only (F152). Per list, in one call, the replacing flag
applies first, then every remove, then every add, each in the order given, so a
remove then an add of the same item moves it to the end. A remove of an item the
list does not hold, or an add of one it already holds, refuses and writes
nothing. `data` is `{project, id, path, commit, paths, routes, lists}`, `routes`
as for `add`; `lists` has one `{list, before, after, removed, added}` per list
the edit changed (`list` is the flag name), and the human view prints each item
removed and added, then the list before and after, every item fenced.

| Refusal | When |
|---|---|
| `E-RS-DIRECTORY-ROLE-MISSING` | the role has no `role.dd.json`; the next step is `pij directory add` |
| `E-RS-ARG` | no id, no `--why`, nothing to change, an edit that changes nothing, `--set` without `=`, a field `--set` cannot set, a field given twice, `--file` with flags, a document naming another id, `--set` on any verb but edit, `--add-<list>` or `--remove-<list>` on any verb but edit, a remove of an item the list does not hold, or an add of one it already holds |
| `E-RS-DDOCS-INVALID` | ddocs refuses the edited document; nothing is committed |

`pij directory retire <role> --why <text>` and `pij directory merge <from>
--into <to> --why <text>` reshape the community as the work changes (req-28,
C26). Retire sets the role's `meta.state` to `retired` with a changelog row.
Merge retires `<from>` with `meta.merged_into: <to>` and adds a changelog row to
both roles (`merged into <to>: <why>` and `<from> merged in: <why>`), in one
commit. Nothing is moved or rewritten: a retired role's `role.dd.json`, memory,
outputs and holders lineage stay where they are and stay readable. The view
gives each role `state` (`active` or `retired`), `merged_into` (or null) and
`merged_from` (the roles merged into it, derived from their `merged_into`), and
the human view prints `retired, merged into <to>` and `merged from <from>`. A
retired role no longer clears a core-role prompt (C22) and routes nothing: the
same commit rewrites `ROUTES.md` without its rows (plan 202). `data` is
`{project, id, state, merged_into, commit, paths, routes}`. Through the shim a
successful retire or merge prints one line (plan 215, F89): `retired <role>
(commit <sha>)` or `merged <from> into <to> (commit <sha>)`, then `; withdrew <n>
queued request(s) (<m> gone)` when it withdrew any, and `; routes regenerated
(+<added> -<removed>), told <prime>` when `ROUTES.md` changed; `--json` keeps
the receipt. Only retire and merge change a role's
standing: `edit` refuses a retired role and refuses a change to `state` or
`merged_into` (a `--file` that leaves them out keeps them). ddocs cannot tie
`merged_into` to `state: retired` or check that it names a role, so the writer
refuses either on every write (`E-RS-ARG`, `E-RS-DIRECTORY-ROLE-MISSING`), and
`merged_from` counts only retired roles. Once retired, a role takes no sends:
`pij send role:<id>` refuses `E-RS-DIRECTORY-RETIRED`, naming the merge target.

Requests queued for the role (no live holder took them) are never stranded
silently. A send to a role and a retire or merge of it share one admission
gate per (project, role): the send holds it from reading the role's standing
until its request is recorded (queued, or `delivering` to the one live
holder), a drain holds it from claiming a queued request until that request is
delivered or back on the queue, and the reshape holds it from reading the
queue through its commit and withdrawal, so no request is admitted between the
check and the retirement. The gate goes with the work that can still land, not
with the caller awaiting it: a send cancelled while its insert waits on the
store, or a retire cancelled while its commit waits on the writer, keeps the
gate until that write has landed or failed. A retire or merge also refuses
while any request for the role is `delivering` (claimed, not yet settled),
since a failed delivery, or a daemon that stops first and re-queues it at
boot, would put it back on the queue behind a retired role. This check only
ever refuses a retirement: it is a brake, not a policy. Without `--drop-queued`, retire and merge refuse and list
the queued requests. With `--drop-queued` the role is retired, each requester
is told once by an FYI held for its next real turn (msg id `<request
id>.withdrawn`, so a repeat is ignored) that names the role, who retired it and
why, and repeats the request's body, and then every request still queued for
it (any instance) is withdrawn in one store transaction: its `role_requests`
row is deleted and a `role.request.withdrawn` spine event, with the requester
as its seat, records `{id, project, role, instance, requester, created_at,
withdrawn_by, why, requester_gone}`. The retiring seat's own requests are not echoed back to
it. `data.withdrawn` lists `{id, requester, instance, created_at, told}`, where
`told` is `held`, `self`, or `gone: <why>` for a requester that is no longer a
seat (unknown or tombstoned: nobody to tell). Those are still withdrawn, listed
again in `data.gone` (request ids), and their events carry
`requester_gone: true`. Nothing is withdrawn unless every other requester's FYI
was held: one that could not be (`not told: <why>`) refuses
`E-RS-DIRECTORY-NOT-TOLD` after the retirement landed, with the requests still
queued, and the same command tells and withdraws them. If the withdrawal
fails after the retirement committed (`E-RS-STORE`, naming the command), the
same `retire` or `merge ... --drop-queued` finishes it: on an already retired
role with requests still queued it makes no commit and adds no changelog row
(`data.resumed: true`, `data.commit: null`), tells nobody twice, and
withdraws them. With nothing queued, a retired role refuses as before.

Every command a refusal prints names the project it was asked about
(`--project <slug>`) and the exact holding or queued instance
(`<role>/<instance>`), so following it from another checkout changes the right
project. Each role, holding, seat and project in it is one shell word, quoted
when it carries anything but `[A-Za-z0-9_-./:@%+=,]`, since a holding instance
can arrive as data (`assign --batch`) and must never become shell syntax: the held refusal prints one release per holding, the queued refusal
one assign per queued instance and the `--drop-queued` form, and F19's
template-diff refusal its edit. Merge does not move queued requests to `<to>`:
a queued request drains only on an assign, so a moved request would wait even
when `<to>` has a live holder. Drain them with an assign first, or withdraw
them.

| Refusal | When |
|---|---|
| `E-RS-DIRECTORY-ROLE-HELD` | a seat still holds the role; the next step names the release |
| `E-RS-DIRECTORY-ROLE-QUEUED` | requests sent to the role wait queued for a holder ([Send to a role](#send-to-a-role-plan-199)) and `--drop-queued` was not given. Each is listed as `<id> from <requester> for <role>[/<instance>] (<age> old)`; the next step prints an assign per queued instance (which drains them; release it and retry after) and the same verb with `--drop-queued`. Also while a request for the role is being delivered, listed as `<id> from <requester> to <holder> (<age> old)`; the next step is the same verb once it settles (`--drop-queued` cannot withdraw a claimed request) |
| `E-RS-DIRECTORY-NOT-TOLD` | `--drop-queued`: a requester's FYI could not be held, so nothing was withdrawn; the retirement has landed and the next step is the same command |
| `E-RS-DIRECTORY-RETIRED` | the role is already retired, or is the merge target; assign and edit of a retired role refuse too, naming the merge target when there is one |
| `E-RS-DIRECTORY-ROLE-MISSING` | either role has no `role.dd.json` |
| `E-RS-ARG` | no `--why`, `merge` without `--into`, a role merged into itself, `--into` on any verb but merge, or extra flags |

Cost: one writer commit per edit, retire or merge, the same as an add, plus
one read on the `role_requests_queued` index and one on the (normally empty)
`role_requests_delivering` index for retire and merge, and with
`--drop-queued` one store transaction and one FYI per withdrawn request. The
send path reads the role document it already reads. The gate is an in-process
lock: no launch, loop or timer. A drain holds it per request while that
request is delivered, so a send to the same role waits for at most one
delivery.

### The prime's operating picture (plan 203)

`pij directory` names the project's designated prime (`data.prime`, the seat
the project records; `null` when none), and `data.prime_document` says whether
the prime role's document exists, so a project without one reads as that
document missing (the human view: `prime: <seat> (no prime role document
yet)`), not as a missing prime. The prime's prompt carries `note`, naming the
prime.

Suggestions nothing points at (F39), the core and security roles from s03 with
no workstream step needing them, are not listed one by one in a bare view: it
carries `data.suggestions: {roles, command}` (the human view prints one line
naming them) and `command` is `pij directory --suggest --project <slug>`, which
lists each in full in `prompts`. A prompt that a need or another C28 signal
points at (`needed_by`, a `needed` or `recurring` prompt) always shows.
`--suggest` belongs to the view; on any other verb it refuses with `E-RS-ARG`. Each
role also carries:

- `live[].seat_live`: whether the holder's seat is registered and not tombstoned;
- `designated`: for the `prime` role only, the project's designated prime
  `{seat, seat_live}` (its `prime_id`), else `null`. The prime is designated,
  never assigned (C24), and the designation is its only authority (F117,
  review of #177). The human view always prints it, as `<seat> (designated
  prime)` or `no designated prime`, and only its seat's liveness counts as the
  prime role's live holder for the idle review and `call_on`;
- `legacy`: for the `prime` role, each open legacy prime assignment row
  `{seat, instance, since, command}`. Such rows are unsupported and count for
  nothing: they are not the prime role's holder (its `live` is always empty),
  not liveness and not activity. The human view prints `legacy prime
  assignment, ignored: <seat> ...; release with <command>`. The lineage still
  shows them. A waiting request for the prime with no live designated prime
  gets `pij project set <project> --prime <seat>`, never an assign;
- `requests`: `{count, oldest_at}` of its queued requests (s05), or `null`;
- `latest_output`: its newest output (s06) `{kind, subject, head, result, writer, at, path}`, or `null`;
- `memory`: `{notes, latest}`, the number of memory notes (s07) and the newest
  dated note's `yyyy-mm-dd`;
- `call_on`: for a cold role with memory (no live holder, at least one note),
  the command that calls on it, `pij send role:<id> --body-file '<path to what
  you need from it>' --project <slug>` (ruling C30: a cold specialist with rich memory is someone to call
  on, not to retire); otherwise `null`.

`data.actions` lists what needs the prime (req-12), each
`{role, condition, says, commands}` with runnable commands that name the
project:

| condition | when | commands |
|---|---|---|
| `no-live-holder` | requests wait and no holder's seat is live: one action per `instance` of the waiting requests (`""` for any holder, which waits only while the role has no live holder at all) | `pij-rs revive <last holder of that instance>` (when there was one), `pij directory assign <role>[/<instance>] '<seat>' --why '<why>'`, so the assign drains those requests |
| `two-live-holders` | a one-holder role has two or more live holders | names each holder with its unanswered delivered requests (a `--reply none` request expects no answer and is not counted, F150); while two or more have open work, nothing (a deliberate second holder, plan 215, F90); else `pij directory release <role>[/<instance>] <seat>` for each but the one with open work, or the newest |
| `memory-stale` | the role's last two closes (`picture::MEMORY_CLOSES`) both fall on a day after its newest memory note, or it has none | `pij directory memory add <role> --title … --hook … --body-file '<path to the note>'` |

A missing core or security role stays in `prompts` (s03). A workstream step
that is not complete and lists `needs: ["role:<id>"]` adds `needed_by`
(`<workstream> <step>`) to that role's prompt, or, for a role that is neither there nor prompted, a prompt of
kind `needed`: from its template when one ships, else as a new definition with
`<title>`, `<what it always does>` and the standing packet (`--packet`, which
`pij/role` requires) to fill in. Adding the role clears it.
`data.notice` always asks the prime to notice a need no role covers, in the PEC
wording.

Step documents are found by one helper, `picture::step_docs` (ruling C29):
the project repo's `docs/project/workstreams/*.dd.json`, or, while that folder
holds none, the older `docs/project/roadmaps/*.dd.json`. `needs` names roles
(req-24); `depends_on` is the ordering, which the view does not read. Each call
reads and parses those documents once (`picture::read_step_docs`) for both the
needs and the recurring paths.

#### Growing the community (rulings C28, C30)

Every recommendation to create, split, merge or retire a role is a
`picture::Recommendation`, which cannot be built without the signal it rests on.
Each one in `prompts` (create) and `reviews` (split, merge, retire) carries
`act`, `signal: {code, says}`, `command` and `commands`, and may carry `detail`.
`in_order: true` marks `commands` as steps (release, then assign) rather than
alternatives. `later` holds lines for verbs that have not shipped: shown with a
`note`, never in `commands`. None use it today: s15's `retire` and `merge`
shipped in plan 206, so a `mostly-idle` review offers both as `commands`.

| signal `code` | says | used by |
|---|---|---|
| `recurring` | the same system or paths keep coming up across tasks | a path built by at least three workstream steps (`RECURRING_STEPS`; each step's `builds` resolved to its blueprint component's `where`) that no role's `scope.paths` covers: create its owner |
| `re-derived` | knowledge is re-derived (the same finding twice) | taught in the notice |
| `independent-judgment` | a judgment must be independent of the author | the reviewer and security prompts |
| `two-domains` | a seat is bloating because it carries two domains | a live seat holding two or more distinct roles (instances of one role are one domain): split each later role onto its own seat, releasing and reassigning each holding with its instance kept (`reviewer/115`) |
| `one-owner` | a path or system needs one owner | taught in the notice |
| `worth-remembering` | this would be worth remembering: knowledge that should outlive the seat (ruling C30) | taught in the notice |
| `core` | a core role a PEC starts from (C22); its template is an example | the other core prompts |
| `step-needs` | a workstream step names this role in its needs | the `needed` prompts: `add` for a role that does not exist; for a retired role named by an unfinished step (its document stays, so `add` would refuse) a `retarget` prompt instead, with one `ddocs set --value-json <workstream>#steps/<id>/needs <json>` per step that points the step at the merge target, or at `role:<replaces-<id>>` to fill in for a plain retirement (plan 206) |
| `one-off`, `overlap`, `nothing-to-remember`, `mostly-idle` | the signals against | `mostly-idle`: a role with no memory notes, outputs, waiting requests or live holder and nothing started or ended for 30 days (`IDLE_DAYS`): retire it, or merge it into another (both in `commands`) |

A cold role with memory is never put up for retirement: a role is not a seat.
`data.reviews` is the boundary review the prime runs at a wave end, a compact
or a PR merge. `data.notice` teaches all of it from the same signals. It opens
with why a PEC exists (ruling C30): it builds lasting knowledge of its problem
space, whatever the work (an engineering project, a design workshop, research,
operations), held by specialist roles in memory and outputs that outlive any
seat and are called on later, so a long-running specialist is a feature, not a
cost. Then: recommend roles from need, weighing C28's two truths (small
contexts are cheaper and sharper; an expert in its own seat beats a generalist
carrying the topic), and keep the fleet small but expert; the signals for and
against; that a role is not a seat (roles can be many and cold, live seats
few); the boundary review, at which each holder also writes memory (C30); and
that templates are examples. The human view
prints each recommendation's signal under it.

The human view prints times in local time with their zone
(`2026-11-06 13:07 +10:00`), puts each holder of a role with more than one on
its own line, then the next actions and the prompts.

Cost: per call, one query each for the holdings, the project, the queued
requests and each role's newest output (one window query), and one registry
list; each role's document and memory folder, the repo's workstreams and the
blueprints they point at are read in-process. A one-holder role with two or
more live holders adds one indexed query for its holders' unanswered requests
(plan 215, F90); no other role does. It launches nothing, per role or
per seat, and has no timer or poll.

### Role outputs (plan 200)

`pij output <kind> --role <id> --subject <address>` writes one `pij/output`
document to
`community/roles/<id>/outputs/<yyyy-mm>/<dd>-<kind>-<subject>[-<head7>].dd.json`
through the single writer, in one commit, then indexes it in the store
(migration 0036, keyed by project and path, since a path is relative to its
project's governance worktree) and records its pointer. The kinds are `verdict`,
`deliverable`, `note` and `handover`. A verdict needs `--head <sha>`, the exact
head it is for, and `--result pass|pass-with-notes|block`; other kinds may give
a result such as `failed`. `--note-owed` and `--evidence` repeat (evidence is
JSON: `{"command":…,"exit_code":…,"log":…}`); `--link` names an external review;
the body is `--body` or `--body-file` (pij-rs, which the shim forwards
`output` to, reads the file, or stdin for `-`). A handover needs a body: what is in flight now, so a fresh holder can
pick up. The path is chosen under the writer's lock, so two outputs at once
land as two files (`-2`, `-3` …). The writer is recorded; one who does not hold
the role is written with `writer_holds_role: false` and the answer carries a
`warning`.

`--answers <msg_id>` (ruling C3) names the role request it answers. Anyone may
publish an output, but only the request's recorded holder, a live holder of the
request's role, or the request project's prime may answer it; anyone else is
refused with `E-RS-OUTPUT-NOT-HOLDER` before anything is written, stamped or
sent (next: get assigned, or drop `--answers` to publish without answering).
The request is marked answered (s12's `answered_at`, so its time-limit check-ins stop) and
its requester is told in the reply mode it asked for: `wake` (the default; a
cold requester is woken as an audited forced wake, ruling C4), `fyi` (held for
its next turn) or `none`. A `block` verdict or a `failed` result always wakes.
An output nobody asked for goes as an FYI to the seat above the writer (its
recorded parent when live, else the current prime). One seat is told; the
answer's `data.notified` says who, in which mode, the notice's `msg_id` and
what happened. The `msg_id` is `output-<project>-<commit>`, from the output's
own commit, so two outputs never share one and a replayed notice keeps its id.
A failed lookup of the seat above or a refused forced-wake audit is reported
in `outcome` with its cause (`not delivered: …`); nothing is sent after a
refused audit.

`pij outputs --subject pr:N --head <sha>` lists the outputs about that subject
for exactly that head, so a verdict on an older head answers none once the head
moves; without `--head` it lists every head. `pij outputs --role <id>` lists a
role's outputs newest first (`--limit`, default 50). Reads use the store and
launch nothing.

| Refusal | When |
|---|---|
| `E-RS-ARG` | an unknown kind, a missing subject, a verdict without `--head` or a known `--result`, a handover without a body, bad `--evidence` JSON, `--body` with `--body-file` |
| `E-RS-DIRECTORY-ROLE-MISSING` | the role has no `role.dd.json` |
| `E-RS-NOT-FOUND` | `--answers` names no recorded request |
| `E-RS-OUTPUT-NOT-HOLDER` (403) | `--answers` from a writer that is not the request's recorded holder, a live holder of its role or its project's prime |

Plus every writer refusal above. Cost: one writer commit per output (git 7,
ddocs 2) and one delivery to the one seat told. No timer or poll.

### Routing table (plan 202)

pij builds the routing table, "send it, don't do it", from every active role's
scope (a retired or merged role routes nothing, plan 206):
one row per `scope.paths` entry and one per `scope.rules` entry, each sent to
`role:<id>`, ordered by role id, paths first. It lives in
`community/ROUTES.md`, which carries no holder names, so a change of holder never
changes it. Two roles whose path claims can name one file (their literal path
components agree up to where either ends or turns into a glob) give the warning
`W-ROUTES-OVERLAP: <path> (role:<a>) overlaps <path> (role:<b>)`, listed under
`## Overlaps` in the file.

Every role write through the writer (`directory add`, `edit`, `retire` and
`merge`) rewrites
`ROUTES.md` in the same commit when its bytes change, so a regeneration is never
a commit of its own. Every other directory commit (assign, release, either
batch, lineage add, a close's ends and the holdings `pij delivered` ends) checks
it too, in-process, and carries a stale `ROUTES.md` (one edited by hand, or
written before a change it missed) regenerated in the same commit (plan 215,
F72); a current one is left alone, and no holder change ever changes it. The first one replaces a hand-written `ROUTES.md`; the old
text stays in the branch history. `data.routes` on the add carries `path`,
`regenerated`, `added` and `removed` (rows as `{role, kind, what}`), `overlaps`
and `prime`; overlap warnings are also in `data.warnings`.

Delivery is one held FYI from `pij-bg`, never a wake:

| Trigger | Who | What |
|---|---|---|
| a write changes rows | the project's prime (`projects.prime_id`, else the machine's current designated prime) | the added (`+`) and removed (`-`) rows and the commit |
| `directory assign` lands (each seat of an `assign --batch` once) | the new holder | the table for that project, with live holders; `data.routes` on the assign (per seat for a batch) |
| a `compact` control is admitted (`pij send <seat> --command compact`, `pij compact-self`; a compact the memory turn holds, when it runs) | the compacted seat | the table for each project it holds a role in |
| `pij revive` relaunches a seat | the revived seat | the table for each project it holds a role in, else the project whose repo holds its folder |
| `pij directory routes` | the caller | the table, in the reply |

Assign and release never notify the prime. A seat in no project with roles, a
project with no prime, or a prime that is not live gets nothing, and the receipt
or the daemon log says why; a failed delivery never fails the assign, compact or
revive. The compact's FYI is keyed to the compact's recipient and message id
(`routes-after-compact:<seat>:<msg_id>`), and the FYI store holds an id once in
any state, so replaying a compact request holds no second table, before or after
the first is claimed, while another seat compacted with the same id still gets
its own. Controls never carry FYIs, so the compact's FYI lands in the
seat's first turn after the compact. Compactions the harness starts itself, or a person types,
never reach the daemon and send nothing.

`pij directory routes [--project <slug>]` returns `text` (the table with a Holder
now column), `routes`, `overlaps`, `holders` and `stale` (whether `ROUTES.md`
differs from the scopes). `--write --why <text>` rewrites a stale `ROUTES.md` in
one writer commit (`community: routes`), returning `commit` and `written`; on a
current file it writes nothing and says so. `--write` without `--why` refuses
`E-RS-ARG`.

Cost: regeneration adds no launch to the write that carries it; each delivery
is a few store reads, an in-process read of the role documents and one held FYI,
with no launch. `routes` without `--write` launches nothing. No timer or poll.

Source: [`community/routes.rs`](../../crates/daemon/src/community/routes.rs), [`http/routes.rs`](../../crates/daemon/src/http/routes.rs).

### Role memory (plan 201)

`pij directory memory add <role> --title <t> --hook <h> [--tags <a,b>] --body
<text>` writes one note, a `pij/memory` document (`meta.date`, the daemon machine's
local date, plan 209; `title`, `tags`,
`hook`; `body`), to `community/roles/<role>/memory/<yyyy-mm-dd>-<slug>.dd.json`,
the slug taken from the title (lowercase ASCII letters and digits, other runs
as one hyphen, at most 40, cut back to a word boundary), or given with `--slug`
(lowercase letters and digits joined by single hyphens, at most 40). `--tags` repeats or takes a comma list.
`--body-file <path>` reads the body from a file; the CLI (`pij` hands `directory`
to `pij-rs`, plan 213) reads the file, or stdin for `-`, and sends it as `--body`,
while a raw HTTP caller's path is read by the daemon relative to `caller.cwd`. The same commit regenerates
`memory/INDEX.md`. pij's lines live in one fenced block,
`<!-- pij:generated -->` … `<!-- /pij:generated -->`, rewritten whole with one
line per pij note, oldest first:
`- [<date> <title>](<stem>.dd.md): <hook> (<tags>)`, with Markdown in the title,
hook and tags escaped. Everything outside the block is kept as written (only
pij's own empty-index placeholder goes); the fence, never a line's shape, is how
pij knows a line is its own, so a hand-written line that looks generated is
kept. An INDEX with no block gets one appended. The files are built under
the writer's lock, so two adds at once land as two commits and each keeps the
other's line. The commit message is `community: memory <role>: <title>`, then
the hook and the `Pij-Seat` and `Pij-Project` trailers. The envelope returns
`data.path` (the note), `data.commit` and `data.paths`. Notes are never capped,
pruned, replaced or deleted; there is no delete verb.

`pij directory memory <role>` prints `memory/INDEX.md` (`--json`: `data.index`
and `data.notes`, every note's file name). `--note <slug>` prints one note,
found by file name, stem, or the slug after its date; a hand-written `.md` note
comes back as `data.text`, a pij note as `data.meta` and `data.body`. Reads run
in-process and launch nothing.

| Refusal | When |
|---|---|
| `E-RS-DIRECTORY-ROLE-MISSING` | the role has no `role.dd.json` (reads also accept a folder with `memory/`) |
| `E-RS-MEMORY-EXISTS` | a note with that date and slug exists; give another `--title` |
| `E-RS-MEMORY-NOTE-MISSING` | `--note` names no note (HTTP 404) |
| `E-RS-UNPORTED` | a memory verb other than `add` (`memory delete …`) |
| `E-RS-ARG` | a missing or multi-line title, hook or tag, an empty body, `--body` with `--body-file`, `--body-file -` from a raw HTTP caller, write flags on a read, or a `--note` that matches several notes or is not a plain name |

Plus every writer refusal above. Cost: one writer commit per note (git 7, ddocs
2, as for any one-document write); reads launch nothing.

#### The memory turn

Before a `compact` control or `pij close` of a seat that holds a role, or
held one in its current context, the daemon asks that seat to write what it
learned. It reads the seat's holdings in this context (F130): every open one,
and each released since the seat's last delivered compact. A role released on
its PR's merge is still in the seat's context until a compact clears it. The
cutoff is the admission time of the newest compact that plan 222's round
settled after seeing it delivered (`woken`, `resumed` or `holds-nothing`),
among the seat's 16 newest compactions (`compaction_wakes`, which records
every compact `admit_control` admits, of either queue kind). Queue state never
counts: a refused compact is acked `done` for replay safety, and settles
`undelivered`. With no delivered compact on record, every release counts. Two
indexed reads (migration 0045), both bounded: the seat's compaction rows by
`(seat, seq)`, and its holdings by `(seat, to_at)`, open ones then releases
newest first, 32 at most (`CONTEXT_HOLDINGS`). Past that, the prompt says there
were more than 32 and asks for notes on the ones the seat remembers. A seat that
held nothing in this context is compacted or
closed exactly as before. The daemon delivers one prompt from `pij-bg` naming
each role and project, a released one as `released HH:MM` (local time), and
the `pij directory memory add` command, then holds the action until the prompt
is delivered and the seat next publishes `idle`, or until the bound passes,
after which it proceeds anyway. The bound is one constant, 5 minutes
(`memory_turn::BOUND`), and it runs from the moment the request is held, not
from the prompt's admission. The prompt's admission is itself under the bound:
a transport that holds it (Claude's socket grace) cannot stretch the turn past
the deadline, which then ends as `bound`. `PIJ_RS_MEMORY_TURN_SECS`, a positive integer read
at daemon start, may shorten it; a value above the 5-minute ceiling is clamped
and an unusable one ignored, each with a warning on the daemon's stderr.

The caller is answered at once with `data.held: "memory-turn"` and
`data.memory_turn` (`action`, `holdings`, `bound_ms`, `note`), and the compact or
close runs in the daemon. A held compact's answer is still a control receipt
(`msg_id` the control's own, `outcome` `{"outcome":"held","reason":"memory-turn"}`,
`at`), so every client decodes it as it decodes any send; a held close carries
`data.seat` and `data.process` (the close leaves the process running). So no client's request timeout can cut the hold
short (the Copilot extension gives up after 65 s, Node's fetch after 300 s), and
a seat compacting or closing itself, which is mid-turn, is handled the same
way. A held close therefore has no `tombstoned_at` in its answer: the tombstone
lands when the turn ends. Before the action runs the daemon reads the seat
once and skips the action if the seat was closed, or a revive put another
incarnation in it (its process or pane changed), during the turn: a brake, so
an old action never gains authority over a replacement that reuses the seat
id. A held close also rechecks authority: the closer must still be the seat or
its recorded parent, so a seat reparented during the turn is not closed. Each
skip is named in the `memory.turn` event's `then`. A close then compares-and-sets against that same read, so the seat's
ordinary activity changes during the turn do not block it. The native CLI's
`send --command compact` and `compact-self` print the held receipt with its
`held` and `memory_turn`.

A project, role or instance appears in the prompt only when it fits
`[a-z0-9][a-z0-9-]{0,31}`, the shape of every one in use; otherwise it shows as
`invalid (N chars)`, and a role or project that can't be shown gets the generic
`pij directory memory add <role> --project <project>` command (#184 review,
S184-001). A role's instance is chosen by whoever assigns it, and the prompt is
daemon-authored. A release time shows only its digits.

When the store can't read the seat's holdings, or the lookup takes longer than
2 s (`LOOKUP_BOUND`), the compact or close is neither refused nor run without
asking: it takes a generic memory turn ("pij
couldn't read your holdings; write a note for any role you held in this
context, or end your turn"), and `data.memory_turn` and the event carry
`holdings_unread: true`. That is a brake, not a policy: it can only add a turn,
never block or skip the action, so a seat whose context is full is never
stranded by a store error.

A seat has at most one memory turn pending. A second compact or close for it
while one is held is refused with `E-RS-MEMORY-TURN-PENDING` (HTTP 409); it is
never a second turn or a second action.

Every turn records one `memory.turn` spine event on the seat: `action`,
`holdings`, `prompt` (the prompt's msg_id), `ended` (`turn-end`, `bound`, or
`undelivered` when the prompt was refused or could not be sent), `waited_ms`,
`bound_ms`, a `note` whenever the action proceeded without the turn, and `then`
(`closed`, `compact admitted`, `skipped: …` or `failed: …`). When the bound
passed while the prompt was still being admitted, the action runs at the bound
and the turn keeps the admission: the event is recorded once it settles and
adds `admission`, `delivered-late`, `queued`, `refused` or `failed: …` (plan
209). Nothing is withdrawn. The wait is a live
subscription to the seat's `delivery.outcome` and `seat.activity` events with one
deadline: no poll, no timer, no launch. The turn starts at the prompt's own
`delivery.outcome` in the event stream, never at the send's receipt, so an idle
that was published before the prompt was delivered (and is still buffered when
a direct delivery returns) belongs to the previous turn and is ignored. An idle
is only published on a change from working, so the first idle after that
boundary ends the turn that read the prompt, whether the prompt started that
turn or arrived during it.

Cost: one live subscription per pending memory turn (only for seats that
held a role in this context, being compacted or closed), one delivery, one
spine event and one deadline; no poll and no launch. Anyone else pays the two
bounded, indexed holdings reads.

## Number claims and delivery at merge (plan 196)

`pij claim <kind> --for <subject>` hands out the next number for the project and
kind and records it with the calling seat, the subject and the time (migration
0033). Kinds `plan`, `prd`, `workstream`, `blueprint` and `workshop` are three
digits, `migration` four, and any other lowercase kind (letters, digits and `-`,
starting with a letter; not `release`) three. `roadmap` is `workstream`'s old
name (ruling C29): it claims, releases and records as `workstream`, from one
sequence, scanning `docs/project/workstreams` and `docs/project/roadmaps`
alike, so a number is never handed out under both names. The number is one more than the
highest of every claim for that kind, open or released, and the highest already
in the project's main checkout: `docs/plans/NNN-*` and `docs/plans/archive/NNN-*`,
`docs/project/{prds,workstreams,roadmaps,blueprints,workshops}/NNN-*`, and
`crates/store/migrations/NNNN_*`, read in-process. Any other kind is read from
`docs/project/<kind>/NNN-*` then `docs/project/<kind>s/NNN-*` (files and
folders alike), so a `usage` claim lands past `docs/project/usage/019-*` (F171).
When neither of those exists, `docs/<kind>` and `docs/<kind>s` are read only to
warn: if either already holds the claimed number or above, the claim still
succeeds, with `data.warning` naming the folder and its highest number, also
appended to `data.line`. They never set the number and never refuse. Each claim is one
`BEGIN IMMEDIATE` transaction over a unique (project, kind, number) key, so
concurrent claims get distinct consecutive numbers and projects never interact.
`data.number`, `data.formatted` (`0033`) and `data.floor` (the checkout's highest)
come back with the row. An absent folder counts as empty, but a checkout or
folder that exists and cannot be read refuses `E-RS-CLAIM-SCAN` and claims
nothing: an unread folder is not an empty one. The checkout cannot see unmerged
branches, so numbers handed out before claims existed are seeded once with
`pij claim`.

A claim is released, and kept as history, when its seat is tombstoned (close,
reap and revive all tombstone, in the same transaction), when `pij delivered`
sees its number in the merged PR's files (`delivered #<pr>`), or by
`pij claim release <kind> <number>` from the claiming seat, the project's prime
or a designated prime. A released number is never handed out again.
`pij claims [--open]` lists the project's claims with `released_at`,
`released_by` and `release_reason`.

`pij delivered <pr>` is the verb the merger runs once after a merge (ruling C23;
no webhook, hook or poll). It reads the PR with one gh launch
(`gh pr view <pr> --json number,state,mergeCommit,mergedAt,body,files,url`, with `--repo`
taken from the checkout's `origin` when it is on github.com) and takes the
delivered addresses from the body's `Delivers:` lines: full ddocs addresses
separated by commas or spaces, or `none`. A project whose checkout has no
`docs/project` folder has no rows to deliver, so there a body with no
`Delivers:` line is read as `none`, with a note saying so, and the holdings
still end (F119); a malformed line still refuses, and where `docs/project`
exists the line is required. It records (project, PR, address,
merge commit, recorded by, time, PM) once per address; a re-run records nothing
new and appends no event. When it records anything it appends one `delivered`
event whose seat is the PM. The PM is the live holder of the project's
`pm-<workstream number>` role for the workstreams the step addresses belong to
(`docs/project/workstreams/NNN-*#steps/...`, or the old `roadmaps` folder), else
the live seat given by `--pm`; with neither, `data.pm` is `null` and the line
says no PM was told. `data.pm` is the card actually told: on a replay that
records nothing new it is the PM the first call told (`earlier_pm`), never a
different one asked for now; `data.requested_pm` is the one this call chose, and
a note says when it was not told.

It prints, and returns as `data.edits[]` (`{address, value, command}`), the
`ddocs set` commands for the PM's next docs PR: each requirement's
`delivered_by` gains every PR recorded as delivering it that the cell does not
name yet (a comma-separated list, appended and never overwritten), so edits
printed for several merges before the docs PR each carry all of them; and each
step's `plan` names the one `docs/plans/NNN-*` folder the PR touched. Current
values are read in-process from the checkout. A document that cannot be read
or parsed, a row it does not have, or a cell that is not text gets no edit:
pij never prints a replacement for a cell it cannot see. `data.notes[]` says
what was skipped and why.

It then ends the holdings the PR finished (plan 215, F70): each open
`reviewer/<pr>`, `security/<pr>` and `performance/<pr>`, `coder/<plan>` for
each plan number in the PR's `docs/plans/NNN-*` files (the scan that releases
claims), and `coder/f<N>` for each F-item the body's `Fixes:` line names (F122).
`Fixes: F119, F117` is parsed as strictly as `Delivers:`: each token is `F` and
digits, separated by commas or whitespace. A body with no `Fixes:` line ends no
F-item holding (pij never guesses from the title), and a malformed one refuses
`E-RS-FIXES` before anything is recorded. The plan paths a PR touches select which coder holdings end; they are
not proof the plan's work is complete. Only a holding whose `from` is before
the PR's `mergedAt` ends: the merge time binds the release to the work the PR
ended, so a key reassigned after the merge (new work under the same
`coder/<plan>` or `reviewer/<pr>`) is left open and named in `data.notes[]`,
and a retry after a refused first run still ends what predates the merge.
`mergedAt` is GitHub's clock and `from` is the daemon's, so skew matters only
for a holding assigned within seconds of the merge. With no merge time from gh
nothing ends, and a note says to release by hand. The holdings are chosen from
the store before the writer lock, so each is pinned to its exact holding (its
`from`): under the lock it ends only if that holding is still the open one, and
a release and post-merge reassignment of the same seat and key landing in
between leave the new holding open, named in the notes. They end in one directory
writer commit, `release_why` `PR #<n> merged (<sha>)`, never one commit per
holding and none when nothing is open, so a re-run ends nothing more. `data.holdings[]` has one release receipt each (the
shim prints `released <role>/<instance> from <seat> (commit <sha>)` per line),
the line counts them, and each live seat's window follows what it still holds.
A holding the store lists but the holders document does not is left alone and
named in `data.notes[]`. If the writer refuses, the delivery stays recorded
and the refusal says to run `pij delivered <pr>` again, which ends what is
still open. Cost: one store read, at most one writer commit (git 7, ddocs 2 per
holders document) and at most one tmux rename per released live seat.
Known limit: `delivered` and `release --batch` end holdings under the caller's
inherited any-seat release authority (as a single `release` does), so any
registered seat can run them. That is F88's root, owned by PRD 003 req-10
(plan 223) under ruling C32; this plan adds no authority check.
`pij delivered` changes no project file and pushes nothing.

| Refusal | When |
|---|---|
| `E-RS-ARG` | a kind that is not lowercase, `release` as a kind, a missing `--for`, a number or PR that does not parse, or a flag the verb does not take |
| `E-RS-OWNERSHIP` | release by a seat that is neither the claimer nor the prime |
| `E-RS-CLAIM-UNKNOWN` | release of a number nobody claimed |
| `E-RS-CLAIM-RELEASED` | release of a claim already released |
| `E-RS-CLAIM-SCAN` | the project's checkout, or a folder of that kind's numbers, exists but cannot be read; nothing is claimed |
| `E-RS-NOT-FOUND` | `--pm` names no live seat, or `--project` no project |
| `E-RS-PR-NOT-MERGED` | gh says the PR is open or closed unmerged |
| `E-RS-FIXES` | the body's `Fixes:` line names nothing, or a token that is not `F<number>` (F122); nothing is recorded |
| `E-RS-DELIVERS` | the body has no `Delivers:` line in a project whose checkout has `docs/project` (or pij can't tell whether it has one), or names something that is not a ddocs address (the refusal quotes the whole line and shows the form to write) |
| `E-RS-GH-MISSING` | gh is not installed or `PIJ_GH` is not an absolute executable; the next step installs it and restarts the daemon |
| `E-RS-GH-AUTH` | gh is not signed in for the daemon's user; the next step is `gh auth login` |
| `E-RS-GH` | gh refused (no such PR) or printed something unreadable |

Cost: `claim`, `claim release` and `claims` launch nothing (store reads and one
directory listing). `delivered` launches gh once per call, by absolute path,
counted as `gh.delivered`. Nothing runs on a timer.

Source: [`http/claims.rs`](../../crates/daemon/src/http/claims.rs),
[`community/gh.rs`](../../crates/daemon/src/community/gh.rs),
[`store/src/claims.rs`](../../crates/store/src/claims.rs),
[`core/src/claims.rs`](../../crates/core/src/claims.rs).

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

A close doesn't stop the process. Its answer says so in `data.process` (on a held close too), naming the pane when the seat has one: `pij close retires the seat; its process keeps running. To stop it, its parent runs: tmux kill-pane -t %N`. A parent that wants the process gone kills the pane after the close. The closed seat can't register under its id again (see [owner-close continuity](#registration-and-tombstone-continuity)), so a process left running is a seat with no address, not a seat that comes back.

```bash
pij reap --dry-run --json
# Only an authorized operator deciding to reconcile records runs the mutation:
pij reap --json
```

Eligibility: confirmed process death **or** `proc_start` mismatch, **and**, for a pane-bound record, confirmed absence of that exact pane. A pane whose root process started **after** the seat's own process is a different pane reusing the id, as after a reboot, and counts as absent (`pane: "reincarnated"`). That narrows the brake without adding policy: an older or unobservable pane root still vetoes. Paneless records require the confirmed stale process alone. A **never-bound** record (no process ever registered: no pid, no start; F114) is decided by its recorded pane alone, because spawn creates the window before it writes the row: its **absence** is the deciding input (`process: "never-bound"`, `pane: "absent"`, `reason: "never-bound-and-pane-absent"`). These brakes each keep it: the pane is present; the pane list can't be read without a launch (it is read only over the live tmux control client, never forked, `pane-probe-needs-a-launch`); the read fails (`pane-probe-failed`); or no pane is recorded at all (`process-identity-unavailable`). A live/recycled process is never signalled. Unknown process/pane evidence is reported as `unverifiable` and blocks retirement. Removing that check can only permit the same set or more reaps: it is a **brake**, not retention policy or an age threshold. Dry-run changes neither records nor spine. Execution re-observes and compares the binding before atomic tombstoning so a concurrent revive is not retired from an old snapshot.

The process adapter uses `LC_ALL=C ps -o lstart=PIJ_LSTART -p <pid>`. Absence is specifically exit **1**, empty stderr, valid UTF-8 and **one header line whose trimmed value is `PIJ_LSTART`, with no subsequent line**. The canonical example is `PIJ_LSTART\n`. Exit 0 requires the header plus exactly one parseable process row. Empty stdout, wrong header, an extra blank/data row, any stderr (even whitespace), other exits, signals and parse errors are unknown—not dead. Existing regression names include `exit_one_with_header_only_and_empty_stderr_is_absent`, `successful_probe_without_process_row_is_unknown`, `failed_probe_requires_exact_header_only_output`, `any_stderr_vetoes_both_absent_and_live_protocols` and `other_exits_and_signals_with_header_only_are_unknown`; naming them is not a claim that this docs change ran them.

Sources: [`reaper.rs`](../../crates/daemon/src/reaper.rs), [`proc.rs`](../../crates/harnesses/src/proc.rs).

## Death sweep

The daemon automatically applies the same conservative reap rule every **5 seconds** by default; set `PIJ_RS_DEATH_SWEEP_MS` on the daemon to change the interval in milliseconds. `pij-rs reap --dry-run --json` exposes the same candidates for an equivalent observation. A candidate needs a `Dead` or `Recycled` process incarnation and confirmed absence of its recorded pane (paneless records use the existing stale-process rule), or, for a never-bound record (no process ever registered), confirmed absence of its recorded pane read over the live control client (F114). Unknown evidence is not death. A healthy seat never takes the never-bound path: registration records its process, after which the two-witness rule applies. A never-bound seat whose pane **id** now belongs to a different pane (tmux reuses `%N` ids after its server restarts) is kept: with no process start to compare against the pane's root, any present pane is a veto. That errs toward keeping a dead seat, never retiring a live one. An operator retires it explicitly: its live parent runs `pij close <seat>`, and when its parent is gone, `pij link <seat> --parent <you> --role worker` first takes it, as `POST /v1/link` allows for a seat whose parent is absent or not live. A close only tombstones the row. It never touches the pane that now carries the id, or any process.

Each unchanged candidate receives one atomic `seat.tombstone` with `reason: "observed-dead"` and observation fields `pid`, `proc_start`, `pane`, `pane_present: false`. Already-tombstoned rows are not retired again. The sweep never signals a process or tears down a pane.

After tombstoning, the daemon pushes an obituary to the child's **recorded parent** through `DeliveryService::send`. It uses `pij_core::BG_ACTOR`, the shared daemon-owned `pij-bg` virtual actor already used for background-job completion—not a second sender identity. The exact text is pinned as `death_notice_body_template` in the [stable route corpus](../../crates/testkit/fixtures/golden/api/governance-routes.json):

```text
[pij] seat <id> died (<reason>) — pane <pane> absent, pid <pid> gone at <iso>. It was your child; revive with `pij-rs revive <id>` or leave it.
```

A never-bound seat's obituary says `it never registered a process` in place of `pid <pid> gone`.

A send receipt proves only its recorded delivery strength: queued is not parent observation. Parents need not poll for child deaths. A parent that is tombstoned or itself observed-dead in the same sweep receives no notice; the daemon counts these suppressions in one line, `death sweep: N notice(s) withheld — recipient dead too`, rather than flooding dead parents after a reboot.

Obituary preparation, send and audit failures are isolated per seat: log the error, continue with the remaining parents, then report `death sweep: N notice(s) failed` once. A failed obituary does not roll back retirement or gain an automatic retry.

## PA watchdog

The daemon runs a watchdog for **PAs only**: seats whose asserted role is `pa`. No other role is watched, primes included. It has **no fixed cadence** (plan 172, 2026-10-13). Every 60 seconds it checks each PA's fleet and nudges each PA on the **idle edge**: the round in which the fleet's **workers** go from someone `working` to no one working. Only the daemon's own turn observation (`SeatDescriptor::state == working`) counts.

**Supervisors never count** (#52 F1): every `pa`-role seat, every `prime`-role seat and the designated prime in scope are supervisors. Their turns are answers to nudges. If they counted, two PAs in one repository would re-arm each other's episode forever, and a lone PA would re-arm its own. Each PA gets one nudge per worker idle episode, and a new episode starts only after some worker has worked again. While any worker works, nothing is sent. The edge state is in-process: a daemon's first look at a PA only arms it, so a restart, or a PA appointed to an idle fleet, sends nothing until the next edge.

The nudge goes out promptly at the edge, while the PA is most likely still warm. It is **not held for a cold or long PA**, because waking the PA is the point. It goes through the ordinary delivery path, so the composer and typing gates still apply. When the send guard's own verdict (`http::cold_wake::verdict`) says the PA is cold, the wake is audited before it is sent, exactly like a forced `pij send`: a `send.cold-wake-forced` event with reason `pa-watchdog: fleet idle`.

**Prime-active wake** (plan 173). After an edge, each PA is woken **once more** when its prime starts working on its own. The prime is the current designation if that seat is live in scope, else the lowest-id `prime`-role seat in scope.

- **Arming.** A per-PA `pij_core::watchdog::PrimeActive` tracker arms at the first round after the edge where the PA and the prime are both observed `idle`. It needs positive evidence: `unknown` or any other state never arms.
- **Firing.** It fires on that same prime's next `working` round. The arming belongs to the prime observed idle, so a different prime, or any other observation of it, drops the arming.
- **Brake.** A `delivery.outcome` `delivered` receipt to the prime since arming is a **brake**: removing it only sends more. Nothing is sent, the trigger disarms, and the cap is kept, so a report delivered after arming never spends the wake.
- **Cap.** At most one wake per PA per worker-idle episode, whoever the prime is.
- **None sent** while the scope is stopped on a usage limit, and none after a restart until the next edge.
- **Body.** `[pij watchdog] <prime> is active again after the fleet went idle: resume your plan.` followed by the digest. A cold PA is audited with reason `prime-active`.

The full behaviour, state diagram and known residual are in [pa-watchdog.md](pa-watchdog.md).

**Fleet.** A PA's fleet is every live seat whose folder resolves, through `git rev-parse --git-common-dir`, to the same repository as the PA's anchor, which covers every worktree. The anchor is the recorded parent's folder whenever the parent has a registry row, even a tombstoned one. Only with no recorded parent, or no row for it, does the PA's own folder anchor it. A folder git cannot place matches only itself.

**Nudge.** The PA receives one message from `pij_core::BG_ACTOR` (`pij-bg`), built by `pij_core::pa_digest`:

- a headline with the scope, seat, working, idle and ❄ counts. The scope is the repository's common git dir, or a seat's folder git can't place. A path has no closed grammar, so it is fenced (`scope "<path>"`: escaped, quoted), and a folder or spawn `--cwd` with a control character or line break is refused where it is set (admission `UnprintableFolder`, F133);
- a **Needs a look** list:
  - seats `SeatDescriptor::nudgeable` that have been quiet for a whole interval;
  - seats that declared `waiting`, `hold`, `blocked` or `question`;
  - stale cards (`status-stale`). These come from the anomaly authority's own `StatusStaleDetector`, run over bounded reads. Each fleet seat gets one indexed `LIMIT 1` read for its latest card. Only for a card that is stale, the round then looks for its own clear or ack, matched by occurrence key, among the seat's `anomaly.clear` and `anomaly.ack` events since that card. That search uses a seat- and kind-indexed window of at most 4 pages of 256 rows. A clear for a different anomaly on the same seat never stands in for the card's own clear. Rows read per round follow the fleet and its stale cards, never the history. Other anomaly kinds stay with `pij anomalies`;
  - refused pane taps (`tap-refused`, F112): a seat whose pane's tap the pane observer couldn't attach, such as a pane already piped with no `@pij-tap-sink` owner marker. The detail is the adapter's refusal, which names the manual repair (`tmux set-option -p -t <pane> @pij-tap-sink '<sink>'` when the pipe is the daemon's own `cat >> <sink>`). It is an in-memory read of the observer's record, no launch. The observer retries such a pane with a backoff that doubles from its interval up to 60 s (`ATTACH_BACKOFF_MAX`), logs it once per pane incarnation, and drops it when the pane attaches or leaves;
  - stale extensions (`stale-extension`, F113): a seat whose extension source differs from what the daemon was built with. It compares content, not commits. Each extension reports ` tree:<id>` in its `extension_build`: ten characters of its directory's git tree id, and for the OMP extension also of the one file it loads from outside that directory (`crates/core/tests/fixtures/control-commands.json`), `+`-joined. `just rs-install` and `just bounce-rs` stamp the daemon with the same (`PIJ_EXT_TREE_COPILOT`, `PIJ_EXT_TREE_OMP`, one `git rev-parse` per build; `pij-rs ping` shows the build's `commit`). A merge that doesn't touch an extension (docs only, or a daemon-only change and bounce) flags nothing. Copilot seats compare with the Copilot stamp, OMP and Pi seats with the OMP stamp; Claude and Codex seats report no extension. A seat with no tree (an older extension, or a `hash:` build) or a daemon without the stamp is not compared. `crates/testkit/tests/extension_closure.rs` fails if an extension's import closure leaves its directory anywhere the identity doesn't cover. `extension_build` is the seat's own report and the digest goes into the PA's prompt, so the daemon parses it against a closed grammar (the build: a 7-40 hex commit with an optional `+dirty`, or `hash:` and 12-64 hex; the tree: one to four 7-40 hex ids joined by `+`) and renders only the parsed ids, labelled seat-reported. A tree that doesn't parse is not compared; a build that doesn't parse shows as `unparseable (N bytes)`, and a whole `extension_build` longer than the grammar allows (238 bytes) isn't compared or parsed further. That applies to stored rows too, since it runs at render time. `extension_closure.rs` reads every quoted `./` or `../` string, so a comment or line break inside an import can't hide it. A computed relative path fails the check, and so does any relative literal that leaves the extension's directory without naming a listed outside file (a concatenation like `"../../../crates/" + ...`). A path built with no escaping literal at all (a variable, `join(dir, "..")`) is beyond a lexical scan. The detail names both trees and the fix: reload the seat's extension, revive it for changed launch flags, or `just bounce-rs` if the extension is the newer. It reads the seat rows the round already has, no launch;
  - lapsed native receivers (`receiver-lapsed`, F124): a live Copilot seat with native extension delivery whose receiver lease has lapsed, so every send to it is refused with `native-extension-unavailable` until the extension recovers. The detail says how long since this incarnation last renewed, and the fix: reload the seat's extension (`extensions_reload`), whose log's `receive-held` line, if any, names the cause. It is one read of the daemon's in-memory lease map per digest, no launch and no store read. The same check decides the refusal, so the two can't disagree;
- **Messages held behind a busy turn** (plan 214, AC2): every message to a seat in this fleet that waited more than 15 minutes (`HELD_FLAG_SECS` in `pij_store::held`) between being queued and reaching the session, with sender, recipient, msg id, held time and whether it is still queued. A delivery's held time is `jobs.acked_at - jobs.enqueued_at` on every rung (the ack is written after the body reaches the session: the Copilot extension's consume, an OMP/Pi steer, a PTY type, a socket ack), so no column was added. The ack that ends a long hold records it in `held_deliveries` (migration 0043); the digest also lists messages still queued past the threshold. Each is shown once: every digest that carries the list (the idle-edge nudge, the prime-active wake and the dormant-PA wake) marks what it listed once its message is admitted (F115: the dormant wake once didn't, and repeated the same holds every 30 minutes). One read per digest, scoped to this fleet's recipients before its limit of 50 rows (so another fleet's holds never fill the page), through the unflagged partial index and `jobs_serial`; never per seat.
- every seat, largest context first, with role, turn state, declared state, context, idle time, cache warmth and the cold-wake price of waking it.

Seat ids, roles and msg ids are seat-reported, and the digest goes into the PA's prompt, so each is rendered only when it fits its grammar (seat ids as above, roles in the closed vocabulary, msg and request ids of letters, digits, `.`, `_`, `:` and `-` up to 128); anything else shows as `invalid (N chars)` (F127). This runs at render time, so stored rows are covered too.

The same rule covers every daemon-authored message (F132, `pij_core::render`), not only the digest: role-queue wakes, request frame headers, time-limit check-ins and notices, close and withdrawn notices, output notices, watchdog and presence messages, modal-stall notices, the usage-reset wake, memory-turn prompts, compaction wakes, the routing table and the directory picture. A role address shows its instance only when it fits the name grammar (`[a-z0-9][a-z0-9-]{0,31}`, `pij_core::render::is_name`, the one the memory turn uses), because a broad send copies the holding's stored instance into the request; the several-holders refusal never advises an instance it can't show, and names a seat instead. Seat ids, request and msg ids, roles, heads, models, result words, subjects and directory names each show only when they fit their grammar, else `invalid (N chars)`. A reason a message exists to carry (a close's or withdrawal's `--why`) is fenced: escaped onto one line in quotes, at most 500 characters, and named as its author's words. Request bodies stay as written, under a header that names their requester.

The human `pij directory` view renders the same way: holders, instances, machines, the prime, role names and the latest output show only when they fit their grammar, and a legacy prime row whose seat or instance can't be shown gets no release command (`command: null`). A template drift line prints its `template-diff` only when the template, role and project all fit (a project is the slug `pij project create` makes, at most 48). The other plain renderers (`pij requests`, `pij outputs`, assign, release and lineage receipts, the claims list) render the same way, and a request's close reason is fenced. `--json` keeps every stored value as it is so an operator can still address a legacy row: its identity fields (seats, instances, machines, request instances) are raw, untrusted data, never text to follow or paste into a shell unchecked.

The receipt lines the daemon builds render the same way (F142): a request close or withdraw line shows the request id and closer through their grammars and the reason fenced (`request <id> closed by <seat>, who wrote: "<why>"`), as do its retries and refusals; a claim line fences its subject and renders its claimer; `pij delivered` renders the PM seat; and plain `pij state <id>` renders the seat's reported model and its delivery notes. The claims listing fences each claim's subject and release reason. When a close's notice can't be held, its NOT-told line names why in a closed vocabulary (the requester's seat is not registered, is closed, can't take messages, and so on), never a delivery error's text. A `bg` job's title and output path are fenced in every line, and a parent tailing its child's job sees each output line fenced. A `Delivers:` address refuses control characters, and a plan folder from a PR's file paths is shown or recorded only when it is plain (`docs/plans/` then letters, digits, `-`, `_`, `.`). A pij error's own text renders what it carries, so no refusal or line built from an error's text can carry a line break or a forged frame: seat ids go through the seat grammar, a caller's free text (a tombstone reason, a recorded path) is always fenced, and the daemon's own words and closed values show as one plain line only when they have no control character, no line break (including NEL, U+2028 and U+2029) and no `[pij ` frame marker, else fenced.

The digest reports facts. Policy, such as caps and exemptions, lives in the PA brief (`skills/pij/references/prime/pa.md`).

**Usage-limit reset.** A fleet idle on a usage limit gets no edge nudge, because nothing can run. If any live Claude Code seat in the PA's scope is stopped by a quota rejection when the edge comes, the round records `watchdog.round` `{outcome: "idle-on-limit"}` and sends nothing. The reset wake follows instead:

- **Source.** The source is the seat's transcript entry with `isApiErrorMessage: true`, `error: "rate_limit"`, `apiErrorStatus: 429` and `quotaLimits: {status: "rejected", rateLimitType, resetsAt}`, where `resetsAt` is UTC epoch seconds (`pij_core::usage_limit`, `pij_harnesses::ClaudeUsageLimits`). The display text ("resets 3:10pm …") is never parsed, and panes are never scraped.
- **Which entry counts.** Only the transcript's *last* assistant entry counts, read from its final 64 KiB. So a seat that got past the limit, or an old rejection deeper in its history, does not count.
- **Other harnesses.** OMP, Codex, Copilot and Pi record no equivalent structured field that pij knows of, so their seats are not read.
- **When.** There is one wake per `(scope, resetsAt)`, at `resetsAt` plus `LIMIT_WAKE_MARGIN_SECS` (120 s). Every seat stopped by that reset in that scope is named in the one wake. It is claimed in the store's `limit_wakes` table (migration 0030) before it is sent, so several seats, overlapping rounds and restarts never wake twice. A reset that has already passed when first seen wakes at once, and the wake says it was seen late. The first sighting of each `(scope, resetsAt)` is audited as `watchdog.limit-scheduled`, with `due_at_ms`.
- **Target.** The PA in scope; with no PA, the prime. The prime is the current `prime_designation` if that seat is live in scope (`prime-designation`), else a live seat in scope with role `prime` (`prime-role`); ties go to the lowest id. The source is recorded as `target_source`. No target still claims the reset and records `delivery: "no-target"`.
- **Guard exemption.** The wake is exempt from the cold-wake guard with reason `limit-reset`, because the target is cold by necessity: a cold target gets a `send.cold-wake-forced` audit with that reason. The woken seat decides which workers to restart. The watchdog never wakes a worker.
- **Audit.** Each wake appends `watchdog.limit-wake` with `scope`, `resets_at`, `limit_type`, `target`, `target_source`, `reason`, `late`, `stopped`, `cold_check` and `delivery`.
- **Time.** Times are epoch values throughout. The wake shows the reset in the daemon's local zone with its name, rendered at that instant, so a reset across a daylight-saving change shows the zone in force then (e.g. `Wed 07 Oct 2026 03:00 AEDT`).

**Bounded work per round.** Between edges a round reads no spine history. It does one registry list, one role join, and a size and mtime check of each Claude transcript; a transcript is re-read only when it changed. At an edge, transcript reads have a 3s wait, and at most one read per seat is ever outstanding. A read that outlives its wait is kept, that seat shows `?`, and no second read starts behind it until it lands. Spine reads are the per-seat card read and the per-stale-card disposition window described above, plus, only on a would-be prime-active wake, the prime's `delivery.outcome` window since arming (at most 4 pages of 256 rows).

**Presence and large seats** (plan 217). The same round runs four checks per scope (a prime and the live seats in its repository, plus each PA's scope). A scope is active while its prime or a worker was observed `working` in the last 20 minutes, not on a usage limit; fewer than two workers never raise the PA alerts. An active scope with no live PA for 20 minutes wakes its prime once, with the last PA's `pij-rs revive` command or the spawn form. A live PA with no turn (`seat.activity`, or a round that saw it `working`) for 30 minutes while its scope is active is woken once with the digest (cold reason `pa-watchdog: dormant while fleet active`); with no PA turn 10 minutes later, the prime is woken once with that wake's receipt. A fleet seat over 600,000 tokens is named to the prime in a held FYI, never a wake, once per crossing (re-armed at or under the line); crossings in one round share it. Each message is claimed in `presence_claims` (migration 0039, keyed by check and subject) before it is sent and released only on positive evidence, so a restart never repeats one; a refused delivery releases it to retry. Each change of verdict appends `watchdog.round` `{outcome: "pa-missing" | "pa-dormant" | "pa-unresponsive" | "large-seat", verdict: "waiting" | "sent" | "capped" | "armed" | "braked", …}`. No launch, timer or poll of its own: per round one designation read and one claims read, one `LIMIT 1` `seat.activity` read per PA in an active scope, and a session-status read only for fleet seats that are or were just working, have no completed read, or have posted activity since their last read. Thresholds, payloads and what to do on each: [pa-watchdog.md § Presence and large seats](pa-watchdog.md#presence-and-large-seats-plan-217).

Every edge appends a `watchdog.round` event. Its payload carries `outcome` (`nudged` or `idle-on-limit`), `prime`, `scope`, `seats`, `delivery` (the receipt outcome, or the error text), `cold_check` and `prime_active` (`watching`, or `no-prime` when the scope has no prime to watch). Every prime-active verdict other than waiting appends `watchdog.round` `{outcome: "prime-active", verdict: "sent" | "braked" | "capped" | "idle-on-limit", reason: "prime-active", prime, scope}`; a `sent` also carries `seats`, `delivery` and `cold_check`. `PIJ_RS_WATCHDOG_SECS` (default `watchdog_interval_secs`, 20 minutes) is the stall threshold: it decides when a ready seat is "quiet a whole interval" in the digest, and it is the default opt-in interval. It never schedules a PA nudge.

## Modal-dialog stalls (plan 219)

A harness's own question or permission dialog stops a seat's turn while pij still reads it `working` (F47: two verdict seats sat on Copilot's "Choose an optional follow-up" dialog for about two hours each). The PA watchdog's 60 s round finds them; there is no timer, poll or launch of its own.

- **Candidates.** A live seat that is `working`, has a pane, runs a harness with known dialog markers (Copilot and Claude today), and has recorded **no own event for 20 minutes** (`pij_core::modal_stall::STALL_AFTER_MS`). Own events are `seat.activity`, `report.now` and `report.state`; deliveries and audits addressed to the seat don't count, so messages sent to a stalled seat don't hide it. Nor does `delivery.inbox-ack`: the Copilot native extension's reader acks on its own while the model sits on the dialog (F108). A seat with no own event at all is not a candidate. Each working seat with a pane on such a harness costs one `latest_matching` spine read per round: one read snapshot of three indexed per-kind SELECTs. Other seats cost nothing.
- **Read, without a launch.** A candidate's visible pane is read once per round **only over the daemon's already-live tmux control client** (`TmuxPort::capture_without_launch`): it never starts a client and never forks `capture-pane`. With no live client the candidate is not read this round: the skip is recorded as a `watchdog.round` event on the seat, `{outcome: "modal-stall-skipped", reason: "no live tmux control client: not read this round, nothing launched", pane}`, and nobody is told. No key is ever sent.
- **Match.** `pij_harnesses::modal::MODAL_MARKERS` names each harness's dialog chrome and where it must sit, anchored to the bottom of the screen where the composer would be. Copilot: the footer `↑/↓ select · enter accept · ctrl+d decline · esc cancel` directly above the bottom rule and status line, or the title `Copilot needs information` opening the region between the last two rules. Claude: `Esc to cancel` as the screen's last line, below the last rule. The same words quoted in the transcript sit above the composer and never match. A match is named by the marker's **fixed label** (`Copilot selection dialog (ctrl+d declines)`, `Copilot question dialog (ctrl+d declines)`, `Claude permission prompt (esc cancels)`), never by text read from the pane, so the seat's work never reaches a stored row, a message or a view. Fixtures: `crates/testkit/fixtures/harnesses/copilot-follow-up-dialog-{herring,lynx}.txt`, `claude-2.1.284-permission-dialog.txt`, and the no-match `copilot-working.txt`.
- **Tell, once per stretch.** On a match the stall is claimed in the store's `modal_stalls` table (migration 0042) **before** anyone is told, keyed by the seat and the stretch's identity: the spine **sequence** of its last own event (its time is used only for duration, so an own event with an earlier clock still starts a new stretch). Then `pij-bg` sends one normal message to the seat's live parent and one to the PA in its scope (else the prime, chosen as for limit wakes), never the seat itself: seat, harness, how long it has been silent (its last own event, not proven dialog time), the dialog's label, and the key that clears it (Copilot: ctrl+d declines; Claude: esc cancels). Each recipient whose delivery is accepted (delivered, a replay included, durably queued, or held for a human's approval, which must not be re-sent) is recorded in the row's `told`, judged from the structured receipt; one whose tell was refused (a native receiver whose lease lapsed refuses, then recovers) or errored is tried again on later rounds with the same message id (`modal-stall:<seat>:<seq>:<recipient>`), up to `MAX_TELL_TRIES` (5) attempts per recipient per stretch, counted in the row's `attempts` BEFORE each send (when that save fails, nothing is sent, so the bound holds), and a recipient that appears later (a new PA) is told then. A restart or an overlapping round never tells a recorded recipient twice. A cold recipient is woken and audited with reason `pa-watchdog: modal dialog`. Each round that tells anyone appends `watchdog.modal-stall` on the seat with `harness`, `pane`, `quiet_seq`, `quiet_since`, `dialog`, `key` and `told` (each recipient tried and delivery's answer).
- **View.** While a claimed stall stands (the last read still showed it, and the seat has recorded no own event with a later sequence), `/v1/seats` and `/v1/state` carry `dialog: {dialog, key, clears, since_ms}` for the seat. `pij list` shows `blocked on a dialog (<key>)` in its STATE column instead of `working`, and `pij state` adds `blocked on a dialog: silent for <duration>, with <label> on screen (clears with <clears>)`. `state` itself stays `working`, the daemon's observation. Remote seats carry none. Each `/v1/seats` request and each `/v1/state` adds one standing-stalls read.
- **Cost.** No new timer, poll or launch. Per round: one spine read (three indexed SELECTs) per working seat with a pane on Copilot or Claude; for each candidate, one control-client capture (none without a live client) and, only when some seat is a candidate, one read of the `modal_stalls` rows; a write only to claim a stretch, record a recipient, or open or close a row.
- **Adding a harness.** Capture its dialog and a normal working screen, commit both as fixtures with MANIFEST rows, add a `MODAL_MARKERS` row naming where the marker sits, its fixed `label`, its short `key` and the `clears` text, and add match and no-match tests in `crates/harnesses/src/modal.rs`. A harness with no row is never read; OMP and Pi have none yet.

## Compaction wakes (plan 222)

A compaction can end a seat's turn with nothing queued, and nothing wakes it (F111: a coder held its role about 17 hours that way). When a seat that holds work is left idle by a compaction, pij sends it one resume message. This rides the PA watchdog's 60 s round, with no timer, poll or launch of its own.

- **What counts as a compaction.** Every compact control pij admits: a direct `pij send <seat> --command compact`, `pij compact-self`, and the compact a memory turn held. All of them enter delivery through one place, and that place records a pending row in `compaction_wakes` (migration 0044) **before** it admits the compact, keyed by the seat and the control's message id. The write completes even if the caller is dropped, and if it fails the compact is refused (an internal error; retrying the same send is safe), so no compact is ever admitted untracked. A replay records nothing new. A compaction the harness runs on its own, or one typed into the pane by hand, is not seen.
- **When the wake fires.** The round reads only the pending rows, newest first within a seat by **admission order** (the row's `seq`, an AUTOINCREMENT written before the compact is admitted; `admitted_at` is a wall clock that can step back or repeat, so it is used only for how long a compact has waited for delivery). An older row is superseded once a newer compact of the same seat is delivered, and waits while a newer one is still in flight. A read that fails leaves the row pending; it never settles one. The newest row wakes its seat when all of these hold:
  - the compact control has been **delivered** for at least 5 minutes (`COMPACT_SETTLE_MS`; Copilot posts no event when a compaction ends, so this is the bound);
  - the seat is live and `idle`;
  - since the delivery, it has recorded **no own event except `seat.activity` idle** (Claude's `/compact` posts one as it ends) and **no message was pushed to it**;
  - **nothing is queued** for it (no live job of its delivery or control kind);
  - it **holds** an open role holding or an open claim.
- **What it sends.** The row is marked `woken` **before** anything is sent, so a restart or an overlapping round never sends twice. The mark is refused, and the row left pending with nothing sent, if a newer compact of the same seat has been recorded since the round read its rows; the older row can still wake if that one is given up. `woken` means the one attempt was made, not that it was delivered: a failed or refused send is not retried. Its outcome is in the audit event's `delivery`. Then `pij-bg` sends the seat one normal message (`compaction-wake:<seat>:<compact msg_id>`): `resume: you hold <holdings and claims>; re-read your handover and continue, or tell your parent if you're blocked.` Its live parent gets an FYI (`compaction-wake-fyi:<seat>:<compact msg_id>`) naming the seat and what it holds. A cold seat is woken and audited with reason `pa-watchdog: compaction left a holder idle`. Each wake appends `watchdog.compaction-wake` on the seat with `compact`, `msg_id`, `held`, `delivery`, `cold_check` and `parent`.
- **When it doesn't.** A row is settled without a message as:
  - `resumed`: the seat started a turn, reported, or was sent a message after the compact;
  - `holds-nothing`;
  - `gone`: the seat is tombstoned;
  - `undelivered`: the compact was never delivered within an hour (for example, its caller was dropped after the row was written);
  - `superseded`: a newer compact of the same seat was delivered.

  While the seat is inside the settle bound, not idle, or has something queued, the row waits. Once per compaction: a new compact is a new row.
- **Cost.** No new timer, poll or launch. Per round, one read of the pending rows (a partial index, empty in steady state). Each pending row costs two indexed reads per round: its delivery outcome by message id, and the seat's events after it. A row past the settle bound with the seat idle adds one queue peek and two holdings reads (the open-holdings query and the `claims_open_by_seat` index). The row is written once when recorded and once when settled, and the wake sends one message plus one FYI. A row is **pending**, and costs reads, only from its compact until it settles: usually one settle bound, at most an hour for a compact that never lands (one read per round). Settled rows stay as history, one per compaction, outside the pending index.

## Idle holders (plan 224)

A seat can sit idle while it holds work for reasons other than a compaction: a plain send it finished, a missed message, a turn that just stopped (F136: a merger held its role for hours after a plain send, with no role request and no time limit to escalate). Plan 222 only covers the compaction case, and s12's limits only cover role requests. So the PA watchdog's 60 s round also nudges a holder that has done nothing of its own for an hour. It adds no timer, poll or launch.

**Off in a bare daemon, on in the fleet (F144, F147).** The nudge runs only when the daemon starts with `PIJ_IDLE_NUDGE` set to `1`, `on` or `true`; a bare `pij-rs daemon` has it off (`Config::idle_nudge` defaults to `false`). The fleet's own launchers turn it on. `just bounce-rs` and the LaunchAgent that `just rs-autostart` writes both pass `PIJ_IDLE_NUDGE=${PIJ_IDLE_NUDGE-1}`, so it is on unless the variable is set otherwise when they run: `PIJ_IDLE_NUDGE=0 just bounce-rs` turns it off for that bounce, and `PIJ_IDLE_NUDGE=0 just rs-autostart` writes it off into the LaunchAgent. Both recipes accept exactly `1`, `on`, `true`, `0`, `off` or `false` (any case) and pass only `1` or `0`. Any other value is refused before anything is built, stopped or written, so it can never reach the daemon's environment or the plist (review of #196, S196-001). As first merged (#189), the nudge treated a resting long-lived role holder (docs-system, delivery, security, a merger) as stalled work and cold-woke specialists overnight. Since F144's fix it looks only at real in-flight work and never cold-wakes a seat for a nudge.

- **Who is looked at.** Only seats with **real in-flight work** (F144):
  - a role holding whose instance names one task: a PR or plan number, optionally with one letter (`coder/224`, `reviewer/183b`), or an F-item (`coder/f144`), each at most 9 digits (`TASK_NUMBER_MAX_DIGITS`). Real numbers are three or four digits; the bound keeps the grammar closed, so a long digit string is never taken for a task;
  - an open `plan` or `fix` claim;
  - a role request delivered to it and not answered (`answered_at` unset; an answered or closed request leaves `role_requests`). A `--reply none` request expects no answer, so it never counts here, in the mark's re-check, or in the notice (F150).

  A bare long-lived role holding (docs-system, delivery, security, merger, pa, pm-001, performance), a named instance (`questions-context`), and a workstream, PRD or migration claim are a resting specialist's, never work by themselves (ruling C30). They are read once per round: open instance holdings (`community_assignments_open_by_seat`), open work claims (`claims_open_by_seat`) and unanswered delivered requests (`role_requests`, which holds only open requests).
- **When it fires.** A holder is nudged when all of these hold:
  - it is live, untombstoned and `idle`;
  - its **last own event** is at least 60 minutes old (`IDLE_NUDGE_AFTER_MS`). Own events are `seat.activity` other than idle (a turn starting), `report.now` and `report.state`. Migration 0046 indexes exactly those rows (`spine_own_events`, a partial index on `(seat, seq, at)`), so the newest is one reverse seek however many idle records trail it. A message pushed to it is not its own, so the nudge never re-arms itself. A seat with no own event at all is left alone;
  - it owns no queued or running **background job** (one seek on `background_jobs_live_by_owner`, also 0046);
  - it has no **pending plan 222 compaction wake**: that wake owns the seat until it settles;
  - **nothing is queued** for it (no live job of its delivery or control kind);
  - it still has in-flight work when its holdings and claims are read again (only in-flight items are named in the message).
- **Once per idle stretch.** The stretch is keyed by the spine `seq` of the seat's last own event (`idle_nudges`, migration 0046, `PRIMARY KEY (seat, anchor)`). The mark is written **before** anything is sent, and only the call that inserts it sends, so a restart or an overlapping round never sends twice. The insert checks the stretch again in the same statement. If the seat has posted an own event after the anchor, is no longer live and `idle`, or **none of the in-flight work the nudge names is still open**, nothing is marked and nothing is sent. That last check means the same task holding, the same plan or fix claim, or the same request still delivered and unanswered, looked up by its keys through the seat's open-row indexes and the request's primary key. It covers an own event racing the round, and work that ended since the round read it (review of #195). The next own event starts a new stretch, which can be nudged once more after another hour. Like plan 222's `woken`, the mark records the one attempt: a failed send is not retried, and its outcome is in the audit event.
- **What it sends.** `pij-bg` sends the seat one normal message (`idle-nudge:<seat>:<anchor>:<occurrence>`, where `<occurrence>` is 16 random bytes in hex, so no caller can take the id first; the mark, not the id, makes it once per stretch): `idle <N> min while holding <holdings and claims>; resume, report blocked, or release.` The prime of each project the seat holds work in (the project's designated prime, if it is live and isn't the seat itself), else the seat's live parent, gets an FYI (`idle-nudge-fyi:<seat>:<anchor>:<own random part>:<prime>`; never derived from the nudge id, which `/v1/events` publishes before the FYI is held) naming the seat, the idle time and what it holds. **A seat is never woken for a nudge** (F144): the message is sent only when the cold-wake check says the seat is warm (`clear` or `busy`). If it is cold, its context size is unknown, or the check fails, the seat is sent nothing (`delivery: "not sent: not warm"`). Its prime's FYI then says pij did not wake it, because a wake re-reads its whole context. This is a brake: it can only spare a wake. Each nudge appends `watchdog.idle-nudge` on the seat with `anchor`, `idle_ms`, `msg_id`, `held`, `delivery`, `cold_check` and `primes`. Every seat id, role, instance, kind and project in these texts goes through `pij_core::render` (#185: `seat`, `name`, `word`); anything outside its grammar shows as `invalid (N chars)`. Requests to one role address in one project are one item with a count and the oldest's age (`40 unanswered role:pm-001 requests (project pij-fleet), oldest 3 h`), and the items listed stop at `HELD_TEXT_MAX` (1,500 bytes) with `...and N more`, so no nudge or FYI passes 2 KB (F150).
- **How it relates to the others.** Plan 222 wakes a seat 5 minutes after a compaction leaves it idle; this one waits an hour and stands aside while a compaction wake is pending. s12's time limits escalate a role **request** that isn't answered; this one is about the **holder**, with or without a request. One threshold for every role: a seat at work posts own events, so only a stalled one crosses it.
- **Cost.** No new timer, poll or launch. Per round, three reads (open instance holdings, open work claims, unanswered delivered requests), qualified in process, then a lookup of each seat in the round's registry snapshot (no per-seat query and no scan of the seats per holder). Only when some holder is idle: one read of pending compactions (a partial index, empty in steady state). Per idle holder: one own-event seek and one mark read. Per idle holder past the hour and not yet nudged: one owner seek for background jobs, one queue peek, the holdings and claims reads, and one mark write that re-checks the stretch with one range seek on the same own-event index; then one cold-wake check (the existing session-status read the cold-wake guard uses), and one message (only if warm) plus one FYI per prime. In steady state (holders at work) it is the three round reads and one seek per idle holder with in-flight work. Building the two 0046 indexes is a one-time cost at migration.

## Opt-in watchdog

Any other seat is watched only while it is opted in. `pij watchdog on [<seat>] [--every <duration>]` turns it on, `pij watchdog off [<seat>]` turns it off, and `pij watchdog status [<seat>]` lists `{pas, optins}`. The seat defaults to the caller. **Any seat may switch any live seat's watchdog**; the store row (`seat_watchdogs`, migration 0029) records `set_by` and `set_at`.

- `--every` defaults to the PA interval and must be at least `1m`.
- The change is persisted before it is announced. A real change appends one `watchdog.on` or `watchdog.off` event. When the caller is not the subject, the subject is also sent a held FYI that names the change and points to `pij watchdog status`; it opens no turn, and the receipt's `subject_told` reports it. These notices are **coalesced**: until the subject reads its FYIs, they share the id `watchdog-control:<seat>:<seq of its latest fyi.delivered>`, which the store ignores when repeated. So on/off churn leaves at most one pending notice per seat. The receipt's `changed` says whether anything changed. A no-op, such as `off` on a seat that is already off or `on` at the current interval, notifies nobody; it is audited as `watchdog.unchanged`, with the actor and the action.
- Refusals:
  - `E-RS-ARG` (400) for bad grammar, including the old TS leaves such as `disable-all`;
  - `E-RS-NO-SEAT` (404) for a missing or tombstoned seat;
  - `E-RS-WATCHDOG-PA` (409) for a PA, since PAs always have the watchdog.

**When it fires.** An opted-in seat is a stall check, unlike a PA's clock. It is nudged once it has been quiet for a whole interval, measured from the latest of its last event, when it was turned on, and its last nudge. A seat mid-turn, or one that declared `waiting`, `hold`, `blocked` or `question`, is never nudged (`SeatDescriptor::nudgeable`). The nudge comes from `pij-bg`, is audited as `watchdog.nudge`, and always ends with how to stop it:

```text
[pij watchdog] <seat>: quiet 20m (watchdog on every 20m, set by <who>). Keep going if working, and report with `pij report now "<did>" "<next>"`. If this unit of work is finished, run `pij report state done`; if you are idle but available, run `pij report state ready`. No more work coming? Stop this watchdog: `pij watchdog off`.
```

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

**Cheap-model compaction (plan 225).** `pij-rs revive <seat> --model <id>` relaunches on that model instead of the recorded one; it is how a seat goes back to Opus (`--model claude-opus-5-5`). `pij-rs revive <seat> --cheap` is for a cold Claude seat that must come back, at the prime's call: it relaunches the recorded conversation with `claude --resume <id> --model claude-sonnet-5-5` (`CHEAP_MODEL`, never Haiku, whose window can't hold these seats), waits until the seat registers on that model (the spawn wait and its model check), and then the CLI sends it the ordinary compact control as the caller, exactly `pij send <seat> --command compact`, so the memory turn and every other compact gate apply. The seat stays on Sonnet, and its recorded model (`boundModel` in `pij list`) says so. Nothing is ever typed into a pane but the compact; never `/model`, which rewrites the user's default model in Claude's settings. It refuses with `E-RS-REVIVE-CHEAP` (HTTP 409), before any write and in one line, a non-Claude seat and a seat whose prompt cache is not known cold: warm (a normal compact is cheaper there), or unknown, named as `cache state unknown: <reason>` (no session bound, harness sessions unreadable, no transcript found, session read failed, last call unknown, cache TTL unknown, or no session facts within 3s); `--cheap` with `--model` or `--fresh` refuses `E-RS-ARG`. Revive's other rules hold: the seat must be tombstoned or observed dead, so a live seat is closed and its pane killed first. No watchdog, sweep or threshold calls it. Cost: no loop, poll or timer; one relaunch and one compact per explicit use, through the existing launch site.

```bash
pij-rs revive <seat> --cheap --json                      # cold Claude seat: relaunch on Sonnet 5.5, then compact
pij-rs revive <seat> --model claude-opus-5-5 --json      # back to Opus, a separate explicit step
```

Without an eligible override, `E-RS-REVIVE-LIVE` names the observation and exact override command; its remediation does not grant authorization. Shim `pij revive` still refuses `E-RS-UNPORTED`, and native `--print`/`--attach` remain unsupported.

## Double seat: `E-RS-CONVERSATION-LIVE` (plan 174)

One conversation cannot be two seats. When a live seat holds a conversation and another process runs the same conversation (a hand `claude --resume` of a revived seat, for example), the daemon refuses with `E-RS-CONVERSATION-LIVE`:

- **`adopt`** of that conversation from any other process. Claude derives the conversation from the pane process's own session record; other harnesses from a verified `--harness-session`.
- **Every verb resolved by pane** from a pane no running seat owns (no live row names it, only retired rows do, or the store-live row's process is gone), when that pane is running a second copy: `whoami --pane`, `send`, `spawn`, `bg`, `report`, `phonehome` and every other verb that resolves its caller through the identity ladder, plus the Claude hook verbs `activity` and `fyi-claim`. The evidence is a Claude record `<claude home>/sessions/<pid>.json` whose `tmux` ends in that pane and whose `procStart` matches the pid's observed start, naming a conversation that is live as a seat in another pane.

The refusal is HTTP 409, `error: "refused"`, and the code leads the text:

```text
E-RS-CONVERSATION-LIVE conversation `<id>` is live as seat `<seat>` in another process (pane %5); close one copy, then run `pij-rs adopt %7 --harness claude` from the copy you keep — one conversation cannot be two seats
```

`details` is `{code, seat, pane, harness_session, pid, stray_pane, stray_pid}`: the live seat, its pane and pid, the conversation, and the second copy's pane and pid. A pane with no such record, a record whose start does not match (a recycled pid), or a conversation no other live seat holds keeps the plain not-registered text. The record scan runs only on that refusal path; a pane a live seat owns never reads the records.

**The live seat is told once.** The first refusal for a pair (live seat, stray pid plus its start) writes a spine `seat.double-detected` event `{msg_id, seat, pane, pid, stray_pane, stray_pid, proc_start, harness_session}`, then sends the seat one `pij-bg` message through the park notice's send-or-hold path, so the cold-wake guard applies (held as an FYI where a send would be refused). The event is the dedupe record, read by seat, kind and `msg_id`, so repeats and daemon restarts send nothing more. The dedupe is a brake: removing it only sends more. An unreadable spine counts as already sent, and a failed audit write sends nothing.

pij never closes or adopts either copy: the human decides which one stays, closes the other, then adopts from the one kept. The Claude SessionStart and UserPromptSubmit hooks show the refusal to the human and the model, and the status line shows it in red (see [double-seat.md](double-seat.md)).

Revive's `E-RS-REVIVE-CONVERSATION-LIVE` is the pre-launch twin. Plan 174 leaves it unchanged and it does not share this helper: any other row holding the conversation, tombstoned included, whose process is observed `Active`, then the Claude record witness. A failed probe does not refuse there.

## Event stream and cursors

`GET /v1/events` is NDJSON, **not SSE**. The first line is the fixture Hello below, not `type=hello` and not a v2 command envelope:

```json
{"build":"pij-rs 0.1.0","hello":true,"v":1}
```

An Event frame has `type`, `machine`, `cursor`, `event`; inner Event has `v:1`, epoch-millisecond `at`, literal `kind`, optional `seat`, and **`payload` is a JSON STRING**. Decode the outer line, then JSON-decode that string once. Inner `event.seq` is not serialized: advance the appropriate machine's cursor from the frame. The canonical `role-set`/governance payloads use `{actor,action,record}`; seat.put decodes directly to the persisted SeatDescriptor and seat.tombstone to its reason object. Existing report.now/report.state payloads are not renamed. New publications have real timestamps; historical `at=0` rows are not rewritten.

No `since` means live-only. Replay takes a URL-encoded JSON cursor map such as `{"fixture-machine":100}`, exclusive per machine; an integer `since=100` refuses. Native CLI spelling: `pij-rs tail --since fixture-machine=100` (repeat `--since` for each machine). This follows daemon events, **not a peer transcript**. `pij spine events --since 100` is a separate local-spine query and returns its local cursor.

A replay is paged (plan 187): the daemon reads `REPLAY_PAGE_ROWS` (1,000) rows by `seq` before it sends the Hello, and the next page only after the client has taken the last one, so the first frame waits for one page whatever the client's lag, and a client that disconnects stops the replay within the page in flight. After a page shorter than the limit the stream continues live; the live subscription is opened before the first page, and seq comparison drops anything both legs carry. The frame format, the cursor contract and the `cursor_reset` refusal (HTTP 409) for a cursor beyond the spine are unchanged.

**A consumer may see the stream end, and should reconnect from its last cursor.** The daemon ends the body with an error, never silently, when a replay page after the first fails, or when its live buffer (`event_buffer_capacity`) overflowed after the replay so that an event may be missing: it stops at the gap instead of skipping it, and the reconnect replays the gap from the spine. A buffer overflow during the replay is covered by the later pages and does not end the stream. chainglass and the OMP/pi extension already reconnect from their cursors; `pij-rs tail` reports the error.

All producers append through the sole EventBus/store ordering seam, commit before broadcasting the returned seq, and preserve replay/live boundary ordering without duplicate/skipped rows. Caller cancellation after admission does not abandon commit/broadcast; shutdown joins admitted producers before flush. Destructive governance mutations commit state and spine event atomically or neither. Consumers must not advance a cursor past an unemitted row.

## Unsupported status

This anchor is permanent; refusals and shipped consumers must not depend on an active plan directory. The inventory distinguishes **shim gaps** from native capability. A missing route never enables legacy execution, even when explicitly forced, and a family row does not promise every old leaf or flag.

- **Shim-only gaps / different meanings:** `spawn`, `revive`, `tail` and `daemon` refuse `E-RS-UNPORTED`; native capability is not a grammar-compatible shim port. Use the documented native forms in the [peer](../../skills/pij/references/routes/peer.md) and [ops](../../skills/pij/references/routes/ops.md) routes where available. Native tail is an event stream, not a transcript. [Native revive](#revive) accepts tombstoned or observed-dead ids and the authorized `--assume-dead --evidence` override; `--print` and `--attach` remain unsupported. Native spawn has no legacy layout/task/branch/plan-id flags.
- **List/sessions filters:** shim `pij list` forwards declared `harness`, `folder`, `parent` and `scope=local` query values to GET `/v1/seats`. Shim and native `pij-rs list --here` scope to caller cwd; path-valued `--here` refuses. Native list has no other filters. `pij sessions` routes GET `/v1/shim/sessions` with no query flags or legacy union. Unsupported `--role`, `--prime`, `--archived` and tree semantics refuse instead of being dropped. Prime designation has no dedicated list/getter projection here: use actual designation receipts/events and authoritative government/human evidence; role assertion or an empty filtered view cannot prove absence.
- **Baton projection limits:** a blocked-time field is not provided. Automatic request notices and their honest delivered/queued/unverified/null projection are supported as described under [Baton leases](#baton-leases); a durable request alone still does not prove recipient observation.
- **Unported command surfaces:** `agent`, `path`, `telegram`, `models`, `watch`, `unwatch`, `chore`, `focus` and `tree` refuse through the shim. `watchdog` is ported (2026-10-09) as [`on/off/status`](#opt-in-watchdog); its old TS leaves (`pause`, `exempt`, `disable-all`…) refuse with `E-RS-ARG`. This does not remove already composed sidecar internals; it does not advertise those old administrative grammars as a native port. `link` is ported (plan 166) with a narrower grammar: the caller is always the parent, and `--role` is required.
- **Inbox/admission:** verified external `inbox register` and `inbox --wait [ms]` are [supported](#verified-paneless-external-admission), not gaps. Paneless adopt, unlisted inbox leaves, generic shim `register` and send `--wait` remain refused; send `--file` is served only for `pij-telegram` (one staged file, sent as a photo, video, GIF or document by type; see [pij-telegram](pij-telegram.md#files-photos-videos-gifs-and-documents)) and refuses for every other recipient. Several files, or files with a body, go through `pij-rs sidecar telegram --file … [--body …]`. Never fabricate host process evidence or use a legacy fallback; extension-owned registration/receive stays owned by that extension.
- **Control and bg are supported, not gaps:** remote compact/new/reload, compact-self and bg create/list/tail/kill remain shipped. Controls carry no body; acceptance is not execution. Paneless controls refuse. Copilot refuses new/reload; Copilot compact queues on `delivery-control:<seat>` and is typed into its pane through the same gates as Claude's, and it also waits for a settled idle (`idle` on two reads 250 ms apart) ([plan 186](copilot-native-extension.md)); order across that kind and the native body kind is by row id among due rows, not strict FIFO, in both directions: a compact may run before later pending bodies, and a compact sent just before a body usually runs after it, because the extension claims the body first and the compact then waits for that turn. new/reload require self or recorded parent plus target arming, not a prime role. Never replace refusal with slash text or sendkeys. bg remains daemon-owned detached execution (in the caller's cwd unless `--cwd`; optional `--timeout` ends it with a TIMEOUT turn; `list` shows running time and duration; `--events` makes an event source whose child fires `pij bg emit` / `POST /v1/bg/{job}/emit`, authenticated by its per-job `PIJ_BG_TOKEN` outside the daemon-key ring, batched per `--min-interval`/`--inline-max`, held as FYIs with `--fyi`, and routed to a warm prime or Telegram instead of waking a cold owner) with durable completion injection (a final turn whose owner is gone, tombstoned or no seat at all, waits up to 24 h for a revive, looked at once a minute, then goes once to the owner's warm prime or Telegram with the job's log path, F139), bounded server-side log reads and owner/parent authorization; it introduces no answer queue cancellation.
- **Native commit-trailers data gap:** forwarding is supported, but `commit_trailers::run` currently calls `derive` with no repository-designation input and does not query current assignment. It reads role-joined self/local seats, then uses a complete recorded-parent root as the explicit interim fallback. `Pij-Plan` derives from worktree path, branch or local flow context. Do not claim designation/current-assignment lookup or use stale legacy rows to fill either gap.
- **StateCard:** inspect its `unsupported` notes instead of manufacturing missing telemetry/lifecycle fields. Optional/null role is absence of assertion, not proof that the role port is unavailable.
- **Recovery boundary:** `answer --supersede` is terminal-only and shared-real-authority-only as above. The refusal is intentional; no queue withdrawal exists. Close/reap reconcile records, not processes. An unsupported force flag cannot bypass ownership.
- **Native sessions capability:** there is no `pij-rs sessions` subcommand in this cutover. The shim's session projection is compared with original live GET `/v1/shim/sessions` response bytes, not an invented native CLI surface.
- **Registration CLI boundary:** `register --role` remains native/HTTP-only. Shim `register` refuses `E-RS-UNPORTED` by name; it is not a fallback registration path.
- **Exit-policy convergence:** native and shim refusal exit policies should converge in separate work. Until then preserve the table above; do not silently align statuses while changing JSON routing.

The actual CLI-verb table is [`skills/pij/SKILL.md`](../../skills/pij/SKILL.md); routing enforcement reads that table rather than a test-owned duplicate. Source inventories: [shim route/refusal declarations](../../.omp/extensions/pij/core/generation-routing.ts), [native Command enum](../../crates/cli/src/main.rs), [native attribution helper](../../crates/cli/src/commit_trailers.rs), [inbox parser](../../crates/daemon/src/http/shim.rs), [identity admission](../../crates/daemon/src/http/identity.rs). Final isolated runtime, full-envelope byte parity and production/chainglass evidence belong to the PM/prime, not this document.
