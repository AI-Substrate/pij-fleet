# pij platform — Rust authority and consumer boundaries

The platform authority is the Rust daemon and its durable SQLite store. Consumers
use the served CLI/HTTP projections and the event stream, not files under
`~/.pij`. Those TypeScript-era files are historical state, not a second current
registry or a fallback source when Rust is unavailable.

The field-by-field wire reference, examples and refusal ledger live in
[pij-rs API](pij-rs-api.md). The exact designed cases and executed client captures
live in [the permanent API corpus](../../crates/testkit/fixtures/golden/api/).
This guide explains which authority a consumer should ask; it does not maintain
a competing schema or tell a UI to reproduce daemon derivations.

## Authoritative surfaces

| Consumer question | Surface | Boundary |
|---|---|---|
| Which seats are visible? | `GET /v1/seats`; `pij list --json` | Read the full envelope, including unavailable peers; active views exclude tombstones. |
| What is this node's current state/tree? | `pij node show <seat> --json`; `pij-rs state <seat> --json` | Use the projection and its unsupported notes; never fill gaps from stale legacy descriptors. |
| Which projects, streams and fences exist? | `pij project`, `stream`, `fence` | Rust records are authoritative; worktree files are not an alternate metadata store. |
| Which tasks and decisions need attention? | `pij node show`, `pij decisions`, `pij anomalies` | Let Rust join current parent/assignment/verification state. |
| What changed? | `GET /v1/events`; `pij spine events` | Machine-scoped cursors and immutable committed events. |
| What is the human event view? | `pij spine render --json` | The returned text/cursor is a view, not a legacy `spine.md` write contract. |
| Which native sessions are bound? | `pij sessions --json` | Rust GET `/v1/shim/sessions`, without a legacy union; no native `pij-rs sessions` subcommand. |

Every `/v1` request requires the daemon's bearer key. Machine access is not a
cryptographically authenticated per-seat identity: callers still use the existing
identity/admission service and cannot invent ownership from a role or a pid field.
Do not bind a UI to the SQLite schema, registry files, transport logs, or an
assumed daemon working directory. Direct store inspection can support an
operator's proof; it is not the public consumer API.

## Wire and cursor rules

- Commands return the complete v2 envelope. Preserve `ok`, `command`, `v` and
  every present `data`, `error`, `meta`, `details` or future field; do not unwrap
  data or manufacture optional values. Null and omission are different facts.
- ErrorKind uses the actual snake_case wire vocabulary. Branch on its structured
  category and `details.code`, not prose. Native and shim refusal exit policies
  differ; use the [exit table](pij-rs-api.md#exit-policy).
- `/v1/events` is NDJSON, not SSE. Require its Hello before frames. Preserve
  unknown event kinds and the JSON-string payload rather than replacing it with
  an invented object schema.
- Cursors are per machine and exclusive. Replay/live handoff must not skip or
  duplicate the boundary. A refused/reset cursor is not permission to silently
  read a different generation's log.
- High-frequency change detection belongs on the event stream. Use current
  projections for joins; spawning a CLI or reimplementing derivations for every
  event is not a replacement stream protocol.

## State, roles and responsibility

Roles are explicit assertions in `seat_roles`, projected into identity, roster,
StateCard and phonehome responses. A stale descriptor role does not override
that authority. Omitted role during admission preserves an assertion; null in a
projection means no asserted role. Display role, including `prime`, does not
grant another seat's ownership or remote-control rights.

Use the current recorded parent relationship. Do not recreate the old
`parentId`/`spawnedBy` fallback or advertise `pij link` as a Rust reparenting API.
Open decision responsibility and answer authorization follow the current parent;
the opening event still records the historical parent-at-ask.

`done` is a claim. Verification references the observed done event; a newer done
event becomes unverified again. Anomalies are computed from Rust data, with the
existing relay text and timing rules. Query them unscoped before asserting that
all nodes are fresh: project/folder filters may omit node-keyed status rows.
Do not manufacture context gauges, badge axes, or lifecycle telemetry from
fields that a current projection explicitly does not provide.

## Writes and physical effects

There is one spine insertion/publication authority. Descriptor and governance
events receive real timestamps and payloads; destructive state changes and their
events share an atomic boundary. A committed event cursor is not a claim that a
later external delivery executed.

- Dispatch queued, delivered and packet-SHA acknowledged are separate states.
  Canary success additionally needs nonce-correlated acknowledgement and fresh
  runtime/model evidence; an inert pane or requested model string cannot prove it.
- A baton request is durable before its keeper notice is sent. Inspect the
  returned notice/delivery evidence; creating a request alone proves no recipient
  observation. Lease return/reclaim requires the exact observed lease id.
- Answer retries retain prepared intent. Only the current parent may supersede
  an eligible terminal non-delivered answer, with shared real queue/spine
  authority. An answer in transit cannot be withdrawn.
- Close writes a tombstone; it does not kill a process or pane. Reap requires
  confirmed stale process identity and, when recorded, confirmed pane absence.
  Unknown observations are a brake. Dry-run does not mutate records or the spine.
- Stream close changes its record; do not infer that the worktree was removed.

Never write legacy descriptor/project/assignment files to influence these
operations. An unavailable daemon or unsupported command must fail explicitly,
not cause a new filesystem authority to appear. External/paneless pull seats use
the verified `pij inbox register` admission path; they must not invent a pane or
bind a long-lived seat to the short-lived CLI wrapper process.

## Deployment and source provenance

The operating invariant is a reviewed, verified production build and its intended
configuration, not a guessed source path or a stale diagram of this machine's
symlinks. Record the source SHA and binary identity used for a proof; launch argv
or a ready message alone is not observed runtime/model provenance.

Treat the canonical checkout and managed skill/extension links as live
infrastructure. Develop in owned worktrees; never replace the production checkout,
global links, configuration or daemon from an unreviewed worktree. Skills and
extensions may be read on later invocations even while an already-running daemon
keeps its old code. Use the repository's read-only doctor commands to inspect
managed installation state rather than trusting a historical path enumeration.

Production deployment, restart and chainglass confirmation are explicit owner
actions. Isolated tests and byte captures are not evidence that production was
updated or that a consumer has completed its migration.
