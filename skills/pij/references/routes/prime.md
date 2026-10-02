# prime — govern many agents in one repository

> Route module — sibling-blind except the worker redirect row required by the
> role boundary. Load one pointer below, then stop.

**Job**: govern many agents in one repo: one o-prime seat, stream orchestrators
below it, platform-store facts, and single-writer government documents.

## Role triage

Use the first matching deterministic probe. Do not choose by persona or intent.
Resolve the current id with `pij whoami --json`; read the complete v2 envelope and compare rs seat ids mechanically. `pij list --json` is the unfiltered active rs roster and `pij node show <id> --json` is its governed subtree. List `--prime`/`--archived`, tree and link are unported, not alternative probes. List `--here` scopes to caller cwd, so it can hide stream worktrees and cannot prove no prime exists.

RoleService's `role` is an assertion, **not prime designation**. Inspect current designation receipts and `pij spine events --json` (decode each event's JSON-string payload), project records and authoritative government/human rulings. There is no dedicated rs prime-list/getter projection in this cutover; if those sources cannot establish absence, refuse to bootstrap by inference and ask the owning prime/human. A missing local directory or empty active roster never proves no prime exists. See [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status).

| Probe | Role | Load exactly this next |
|---|---|---|
| Current rs designation evidence names my resolved id, consistent with authoritative government | o-prime | [`../prime/orient-oprime.md`](../prime/orient-oprime.md), then stop |
| Human explicitly seats me, or an unmigrated prose government's authoritative spine names me; reconcile with rs evidence first | o-prime | [`../prime/orient-oprime.md`](../prime/orient-oprime.md), then stop |
| The authoritative roster/brief names my resolved id as a stream | stream | [`../prime/orchestrator.md`](../prime/orchestrator.md), then stop |
| Human/authoritative evidence establishes no current prime here, and project records name no project here | bootstrapper | [`../prime/rituals/bootstrap.md`](../prime/rituals/bootstrap.md), then stop |
| I am a fleet worker with a bounded packet | worker | Stop here; use `/pij pair` for a fleet or `/pij peer` for one colleague. |

Bootstrapper is LAST: require a silent store. A local `.harness/government/`
directory is NOT a signal in either direction. [Store-native](../prime/rituals/store-native.md)
is the default; documents live separately. Resolve `<government-root>` through
[Governance branch — the rules](../prime/rituals/bootstrap.md#governance-branch--the-rules),
never from a main/stream directory listing.

If sources conflict, stop designation and reconcile the rs receipt/event, current caller/state and human ruling; display role alone does not resolve the conflict.

## Ritual index

| Need | Load exactly this |
|---|---|
| Stand up the seat and government | [`../prime/rituals/bootstrap.md`](../prime/rituals/bootstrap.md) |
| **Stand up your PA** (bootstrap deliverable — Jordan, 2026-08-01) | [`../prime/rituals/bootstrap.md`](../prime/rituals/bootstrap.md) §5 → `<government-root>/briefs/pa-standup-recipe.md`, resolved for **pij's repo**, not the consuming repo |
| Record governance in the platform store (ruled default; lazy self-migration) | [`../prime/rituals/store-native.md`](../prime/rituals/store-native.md) |
| Spawn, adopt, canary, brief, or tear down a stream | [`../prime/rituals/kickoff.md`](../prime/rituals/kickoff.md) |
| Request, grant, return, reclaim, or audit a baton | [`../prime/rituals/batons.md`](../prime/rituals/batons.md) |
| File, verify, relay, or digest a report | [`../prime/rituals/reports.md`](../prime/rituals/reports.md) |
| Something just went wrong across seats — record, repair, rule, encode | [`../prime/rituals/incidents.md`](../prime/rituals/incidents.md) |

## Prime invariants

- Government files have one writer; see [`../prime/protocol.md#government-files`](../prime/protocol.md#government-files).
- At each commit boundary, apply `00-routing.md` § C11 (Commit attribution).
- Worktree-local activity is notification-only; synchronization begins at shared
  mutable resources or converging histories; see
  [`../prime/protocol.md#construction-fences-batons-and-landing`](../prime/protocol.md#construction-fences-batons-and-landing).
- An orchestrator seat never runs long blocking subagents in its own session;
  role-address sends; see [`../prime/protocol.md#seat-identity`](../prime/protocol.md#seat-identity).
- Human rulings land in durable government or plan files immediately; questions
  never block (modal UIs forbidden; the context owner asks); see
  [`../prime/protocol.md#human-rulings-and-non-blocking-questions`](../prime/protocol.md#human-rulings-and-non-blocking-questions).
- Every send costs the recipient's whole context: broadcast only to working or warm seats it changes; no acks or FYIs;
  status and rules live in files; see [`../prime/protocol.md#fleet-messaging-every-send-costs-the-recipients-context`](../prime/protocol.md#fleet-messaging-every-send-costs-the-recipients-context).

## Preconditions

- Use `00-routing.md` § C1 for harness mode/adoption and § C2 for canary proof.
- A git repo, tmux, ambient `harness` and the intended rs daemon must be available. `pij-rs ping --json` establishes reachability; native spawn does not imply an automatic legacy daemon start. An adopted seat confirms itself with `pij phonehome`.
- The human names work. The o-prime never invents portfolio items.

## Failure modes

| Signal | Move |
|---|---|
| Identity/ownership refusal while seating | Read `pij whoami --json`, `pij list --json` and `pij phonehome --json`; reconcile observed binding/tombstone evidence without legacy writes |
| Sends queue without delivery | Check daemon health and stale registry rows; follow [`../prime/rituals/bootstrap.md#recovery`](../prime/rituals/bootstrap.md#recovery) |
| Roster, baton book, or live peers disagree | Stop allocation; run the restart audit in [`../prime/rituals/bootstrap.md#recovery`](../prime/rituals/bootstrap.md#recovery) |
| A step needs doctrine not present here | Load one ritual or [`../prime/protocol.md`](../prime/protocol.md), never the whole payload |
