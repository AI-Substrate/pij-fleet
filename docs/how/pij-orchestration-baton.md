# pij orchestration batons

`pij orchestration baton` provides a machine-wide, registry-backed lease for
exclusive resources used by multiple agent sessions. A baton has one atomic holder,
a purpose-carrying request queue, explicit grant/return/reclaim actions, pushed
notices, and an append-only machine log.

The posture is an **honor system**. Any peer may grant or reclaim; the primitive
records facts and makes conflicts visible rather than enforcing authority. The two
hard mechanics are one lease holder at a time and explicit `--repin` acknowledgement
when a SHA-pinned request no longer matches the baton's repository HEAD.

## Commands

The actor is `PIJ_SESSION_ID` when available, then the current registered pane, and
finally `operator` for a shell outside a pij session. Every command accepts `--json`.

```bash
# Define a resource once.
pij orchestration baton define git-index \
  --resource "shared git index and commit slot" \
  --repo "$PWD" \
  --probe "git status --short"

# Queue a request with its purpose and intended return evidence.
pij orchestration baton request git-index \
  --purpose "land plan 036 implementation" \
  --pin "$(git rev-parse HEAD)" \
  --evidence "commit SHA and green gates"

# Inspect the queue, then select any request by id. The queue is discretionary,
# not FIFO: grant the dependency-ready request.
pij orchestration baton list
pij orchestration baton show git-index --json
pij orchestration baton grant git-index --to request-<id>

# If HEAD moved since the request was pinned, grant exits E-PIN. Re-run only
# after reviewing the new HEAD and explicitly accepting it.
pij orchestration baton grant git-index --to request-<id> --repin

# Normal completion frees the lease and notifies the granter.
pij orchestration baton return git-index --evidence "commit abc123; gates green"

# Reclaim is always an explicit human judgment. Evidence is required.
pij orchestration baton reclaim git-index \
  --evidence "holder dead; purpose not completed"
```

`--probe` is recorded with the definition as an operator-facing check; v1 does not
execute arbitrary probe commands. Pin verification is the only built-in repository
probe and uses `git -C <repo> rev-parse HEAD`.

## Queue, grants, and blocked time

Requests are records, not positions:

```json
{
  "id": "request-…",
  "requester": "pij-worker",
  "purpose": "run the shared integration gate",
  "pin": "optional-sha",
  "declaredEvidence": "gate output",
  "requestedAt": "2026-07-11T09:00:00.000Z"
}
```

`grant --to` may select any request id. The atomic `.lease` publication decides the
winner if two granters race; a losing grant cannot create a second holder and its
request remains queued. `show --json` exposes the active lease's `requestedAt`,
`grantedAt`, and `blockedTimeMs`.

## Notices and receipts

Request, grant, return, reclaim, and holder-alert notices use the existing pij
delivery channel. Command output surfaces:

- `delivered` — a live idle pi peer accepted immediate delivery.
- `queued` — a busy pi peer or a healthy daemon-owned peer will receive it through
  the normal steer/daemon path.
- `unverified` — the target is absent/dead, the daemon heartbeat is missing/stale,
  or delivery could not be confirmed.

Store mutations do not depend on the daemon. A daemon-less request/grant/return still
succeeds and reports `unverified`; it never prints a success-shaped delivery claim.

## Holder liveness

The daemon checks active leases against the pij registry:

- dead PID/session → one alert to `grantedBy`;
- working with stale/no activity → one stalled alert to `grantedBy`;
- healthy or unknown → no alert;
- recovery re-arms the latch, so a later dead/stalled transition alerts again.

An alert **never reclaims or edits the lease**. Inspect the holder's purpose and
evidence, then run `reclaim` explicitly if judgment supports it.

## Store layout

The machine-wide root is `PIJ_HOME/orchestration/` (default
`~/.pij/orchestration/`):

```text
batons/<name>.json   definition, queue, last-lease and holder-health metadata
batons/<name>.lease  atomic no-replace single-holder truth
log.ndjson           append-only machine action/alert records
```

Definition JSON is advisory metadata and uses tmp+rename replacement. The lease file
is authoritative and uses a fully written, fsynced temp file plus an atomic
no-replace hard-link publish.

## Human evidence layer

The machine log records timestamps, actors, verbs, lease/request ids, purposes,
pins, blocked time, transitions, and supplied evidence. It does not replace the
human baton book: narrative terms, hazards, verification, and grant judgment remain
hand-maintained evidence. Product code never reads or writes that book.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Command/store mutation completed; inspect the receipt independently. |
| `1` | Baton/request/lease missing, already held, or stale pin requires `--repin`. |
| `2` | Filesystem store failure. |
| `64` | Invalid primitive, verb, flag, name, or arity. |
