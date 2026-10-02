# node — rs governance records, questions and node truth

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: operate projects, streams, fences, dispatches, roles, assignments, decisions, node cards and the attributed spine in the Rust store. See the [stable API contract](../../../../docs/how/pij-rs-api.md) and [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status). `pij` either serves rs or refuses; explicit legacy forcing is no escape. `--json` is the complete original v2 envelope, never a legacy field projection.

## Attribution and roles

Bearer authentication proves machine access, not seat identity. The daemon resolves caller evidence before mutation. `--actor`, when accepted, must equal that caller; it cannot override attribution. Never copy another seat's `PIJ_SESSION_ID` to gain authority.

```bash
pij role <seat> reviewer --json
pij role <seat> --unset --json
pij orchestration role set <seat> pm --json
pij orchestration role unset <seat> --json
```

RoleService stores explicit assertions in `seat_roles` and joins whoami, seats, StateCard and phonehome. An unstamped role is null, not an inferred worker. Omitted role on adoption/registration preserves the assertion; unset is explicit, not an omission. Role assignment is self-or-current-parent at the service boundary; protocol designations still come from the governor. A role is not prime designation and grants no orchestration authority.

## Projects, streams and fences

```bash
pij project create '<description>' [--repo <path>] [--plan <path>] [--prime <seat>] --json
pij project list --json
pij project show <slug> --json
pij project set <slug> [--description <text>] [--repo <path>] [--plan <path>] [--prime <seat>]
pij stream create --project <slug> --slug <stream> [--base <ref>] [--ordinal N] [--root <path>]
pij stream list [--project <slug>] --json
pij stream show <allocation-id> --json
pij stream close <allocation-id>
pij fence set <stream> --paths <a,b> [--shared <x,y>]
pij fence show [--stream <stream>] [--path <path>] --json
```

Records are shared SQLite authority, not `~/.pij/allocations/` or filesystem ledgers. Stream creation uses the real repository reservation/worktree operation; close changes the record, not the worktree. Fences describe intended writes, not permission, and path filters never widen them.

## Packet receipts and attestation

```bash
pij dispatch <seat> --packet <path> [--wait[=MS]] --json
pij ack <dispatch-id> --packet-sha <sha256> --json
pij canary <seat> [--expect-model <model>] [--wait[=MS]] --json
pij attest <seat> --plan-id <id> --json
```

The daemon reads/hashes packet bytes relative to caller cwd and persists the dispatch before existing DeliveryService admission. Queued is not delivered; delivered is not acknowledged. Only the actual recipient can ack the matching packet SHA, and identical acknowledgement is idempotent. Canary requires a real nonce-correlated dispatch, recipient ack and observed runtime/model evidence; descriptor presence is not a pass. `--wait` observes bounded progress rather than inventing completion.

`planId` is an explicit seat assertion, never inferred from a project, cwd or environment. `attest` does not set native-extension-delivery security attestation. Native spawn has no `--plan-id`; attest the resulting seat separately.

## Tasks and reports

```bash
pij task set <seat> '<task>' [--project <slug>]
pij task close <assignment-id> --reason done|cancelled|failed|superseded
pij report now '<did>' '<next>' [--state <word>] [--note '<text>']
pij report question "<what I need from you>" [--assignment <id>]
pij report blocked "<what I am waiting on>" [--assignment <id>]
pij report state <state> [--assignment <id>] [--refs a,b]
pij report clear
pij report verify <seat> [--assignment <id>]
```

Everything under `report` is a first-person claim about yourself. `report verify <seat>` is the supervisory exception and requires the target's **current recorded parent**, never self or an unrelated seat. With `--assignment`, verification binds the latest done naming that assignment; unscoped verification binds the latest done overall. A newer done for the same assignment reopens its unverified-done anomaly. Closing a task does not substitute for verification.

States: `blocked|question|hold|waiting|ready|failed|cancelled|done`. Mechanical liveness/activity is derived, not a writable semantic state; do not invent `working`. Existing now/state outputs and events stay unchanged. Use `--note` with question/blocked; report clear removes a state, not a durable decision. Record now/next at both edges of work (invariant 12), at the seat's own altitude. Query anomalies unscoped when supervising status cards: project/here filters may hide node-keyed stale status.

Self-reported status never gates delivery or inbox claims: `hold` means the seat
is reporting a pause, not closing the inbox needed for the awaited ruling.
No operator mailbox-hold field is set by `report state`. Per-job typing holds,
actual recipient consent and native receiver/context checks remain separate.

Actively working has no semantic state word. If done, run `pij report state done`; do not self-pause a watchdog. Inline markdown is supported in report text (`code`, emphasis and links), but newlines are refused. Shell-quote backticks with single quotes so they are not executed. Did/next cap at 280 characters each and notes at 200 after whitespace collapsing; over-limit text is refused, never silently truncated (`http/report.rs` owns this boundary).

## Durable decisions

```bash
pij decisions [--state open|answered|all] [--asked_by <seat>] [--parent <seat>] --json
pij answer <decision-id> '<answer>' --json
# Current-parent recovery, only after observing a terminal non-delivered intent:
pij answer --supersede <decision-id> '<replacement answer>' --json
```

`report question` persists the question and `decision.opened`; state changes do not silently answer/delete it. Default decisions are open, fleet-wide. Reads and parent filters project the **current** parent; the opened event preserves parent-at-ask. A root question's prime visibility fallback confers no answer authority.

The asker or current parent may answer. Self-answer closes without self-send (`answer_msg_id: null`); another-seat answer goes to the asker via existing delivery. Identical retry does not duplicate push; conflicting answered content refuses. Supersede belongs only to the current parent and only a terminal non-delivered old answer. Pending/running/held/reserved work refuses `E-RS-ANSWER-IN-TRANSIT`; delivered/acked refuses `E-RS-ANSWER-DELIVERED`. **An answer in transit cannot be withdrawn; wait for its terminal outcome.** Elapsed time and ordinary claim expiry do not establish terminal expiry. No queue cancellation or new transport exists.

Supersession needs shared real SQLite queue/decision/spine authority. Mixed/fake authorities refuse `E-RS-ANSWER-AUTHORITY-SPLIT` before replacement, naming both backends. Ordinary questions/answers remain available on supported configurations. The stable API explains durable outcome evidence and replacement sequencing.

## Node views and spine

```bash
pij list --json
pij sessions --json
pij node show <seat> --json
pij spine events [--peer <seat>] [--project <slug>] [--since N] --json
pij spine append --kind <kind> [--refs <a,b>] [--project <slug>] --json
pij spine render [--peer <seat>] [--project <slug>] [--since N] --json
pij anomalies [--here] [--project <slug>] --json
```

Shim `pij list` supports exact `--harness <h>`, `--folder <path>`, `--parent <seat>` and `--scope local`, forwarded to `/v1/seats` without losing values. Both shim and native `pij-rs list --here` select the caller's canonical cwd; `--here` is boolean, not a path argument. `pij sessions` accepts bare or `--json` only. No `--prime`, `--role`, `--archived` or cross-generation union. `node show` joins rs subtree/governance records; unknown nodes refuse. `link`/`tree` are unported: inspect recorded `parent`, never rewrite it through legacy or infer it from spawnedBy. Existing-seat reparenting/root placement is a named gap, not a role mutation.

Spine kinds are literal/open strings. Append attribution is daemon-resolved and irreversible; correct mistakes with a referenced event. `--peer` is a read filter, not append impersonation. Render returns `{text,cursor}` without writing a legacy spine file. Local `--since N` is exclusive; live `/v1/events` instead uses machine cursor maps. That stream begins with Hello (`hello:true`, not `type=hello`), then Event frames with JSON-**string** payloads; see the API before decoding them.

Canonical examples live at `crates/testkit/fixtures/golden/api/governance-routes.json` and `governance-events.json`. They are not new live captures; PM/prime own validation and production/chainglass acceptance.
