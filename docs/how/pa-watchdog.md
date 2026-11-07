# The PA watchdog

The PA watchdog loop (`crates/daemon/src/pa_watchdog.rs`) runs every 60 s (`TICK`). Its fleet wakes go to **PAs only**, meaning seats whose asserted role is `pa`. A PA hears from it in the four cases below and no others. Other paths in the same loop can reach other seats:

- the limit-reset wake goes to the scope's prime when the scope has no PA;
- the prime is woken when its active fleet has no PA, or when its PA does not answer a dormant wake, and gets an FYI when a fleet seat goes over 600k tokens ([below](#presence-and-large-seats-plan-217));
- an opted-in seat, worker or prime included, gets its own stall nudge ([below](#opt-in-watchdog-not-a-pa)).

No path wakes a worker because its fleet went idle.

| Wake | When | Sent from | Reason on a cold wake |
|---|---|---|---|
| **Idle nudge** (plan 172) | the fleet's workers go from someone working to no one working | `pij-bg` | `pa-watchdog: fleet idle` |
| **Prime-active wake** (plan 173) | after that, the prime starts again on its own | `pij-bg` | `prime-active` |
| **Limit-reset wake** (plan 172 R3) | a usage limit that stopped the fleet resets | `pij-bg` | `limit-reset` |
| **Dormant wake** (plan 217) | the PA has taken no turn for 30 min while its fleet is active | `pij-bg` | `pa-watchdog: dormant while fleet active` |

There is no fixed cadence. Every 60 s (`TICK`) a round reads the registry and roles, then decides each PA's case from what changed since the last round. The exact wire contract (payload fields, store tables) is in [pij-rs-api.md § PA watchdog](pij-rs-api.md#pa-watchdog). How a PA acts on a wake is in its brief, [`skills/pij/references/prime/pa.md`](../../skills/pij/references/prime/pa.md).

## The fleet and its supervisors

A PA's **scope** is every live seat whose folder resolves, through `git rev-parse --git-common-dir`, to the same repository as the PA's **anchor**. This covers every worktree. The anchor is the folder of the PA's recorded parent whenever that parent has a registry row, even a tombstoned one (`scope_of` doesn't filter the parent). Only a PA with no recorded parent, or whose parent has no registry row, anchors on its own folder.

**Supervisors never count as the fleet** (#52 F1). Every `pa`-role seat, every `prime`-role seat and the designated prime are supervisors. Their turns are answers to wakes. If they counted, two PAs in one repository would re-arm each other forever, and a lone PA would re-arm itself. The **fleet** is the workers: everything else in scope (`pij_core::watchdog::fleet_working`).

## Idle nudge: the edge

`IdleEdge` tracks one boolean per PA: was any worker working at the last round? The nudge goes on the round where that flips from yes to no. That happens once per **worker-idle episode**. A new episode opens only after some worker has worked again.

- **Restart:** a tracker's first observation only arms it. A restarted daemon, or a PA appointed to an idle fleet, sends nothing until the next edge.
- **Body:** the fleet digest (`pij_core::pa_digest`). It has a headline, a **Needs a look** list, and every seat by context size, with its idle time, cache warmth and ❄ cold-wake price.

## Prime-active wake: the PA resumes when the prime does

Jordan's scenario (2026-10-14): overnight the workers go idle. The PA is nudged once, does its round and goes idle. In the morning Jordan talks to the **prime**, and the PA must resume its plan too. The prime is a supervisor, so its turn re-arms nothing. A separate per-PA tracker, `PrimeActive` in `pij_core::watchdog`, handles this case:

```
           worker idle edge
 (none) ───────────────────▶ Open ◀──────────────────────────────────┐
                              │ a later tick: PA `idle` AND          │ braked / idle-on-limit (cap kept);
                              │ prime P `idle` (records armed_at, P) │ or P not seen `idle`/`working`, or
                              ▼                                      │ the prime is no longer P
                            Armed ── P observed `working` ──▶ verdict ──┘
                                                         │ sent
                                                         ▼
                                                       Spent ── same prime idle→working ──▶ capped
                         (the next worker idle edge reopens: Open; a new prime never does)
```

- **The prime** (`scope_prime`) is the current prime designation when that seat is live in scope. Otherwise it is the lowest-id `prime`-role seat in scope. With neither, nothing is ever armed. The edge's `watchdog.round` audit records `prime_active: "no-prime"` (otherwise `"watching"`).
- **Arming** happens at the first round *after* the edge where the PA and the prime are both observed `idle`. This is positive evidence: `unknown`, `starting`, `stalled`, `stopped` or `dead` never arm (#53 review F002). Any prime turn seen before arming sends nothing. Arming on the edge round itself would let the PA's nudge-handling race the prime's answer.
- **Identity:** arming belongs to the prime that was observed idle (#53 review F001). If the watched prime changes (a new designation, or a different `prime`-role seat), the old prime's arming is dropped. The new prime must itself be observed idle with the PA, and then working. If it is already idle with the PA on the round it is first seen, it arms on that round (#53 re-review R1). A new prime never resets a spent cap.
- **Trigger:** while armed, the round where that same prime is observed `working`. While armed, any other observation of it (not `idle`, not `working`) drops the arming.
- **The causality brake.** At the trigger, the round reads the prime's `delivery.outcome` receipts on the spine since `armed_at`, using a seat- and kind-indexed window of at most 4 pages of 256 rows. If any says `delivered`, a pij message caused this turn: the PA's own report, or any peer's. Then **no wake is sent**, and the trigger **disarms without spending the cap**. It re-arms at the next round where both are idle. This way, the PA's night-time report can't use up the morning wake.
  - **This is a BRAKE, not a policy:** removing it only ever sends *more* wakes. It can never cause one.
  - A read that fails, or that runs out of pages, counts as delivered: it spares the wake.
- **Cap:** at most one prime-active wake per PA per worker-idle episode. Later prime turns in the same episode are audited `capped` and send nothing. Only #52's worker re-arm (a new idle edge) opens a new episode.
- **Usage limit:** while any Claude seat in scope is stopped on a quota rejection that has not lifted ([below](#limit-reset-wake)), the trigger sends nothing (`idle-on-limit`) and disarms, keeping the cap. The limit-reset wake owns that case.
- **Restart:** the tracker starts with no episode. A restart never sends a prime-active wake until a new worker episode opens.
- **Body:** `[pij watchdog] <prime> is active again after the fleet went idle: resume your plan.`, followed by the fleet digest.

**Known residual.** The prime's state is read once a minute, so one race slips past the brake. The PA's report, or any peer message, is delivered just before the arming round, but the prime is still observed idle at that round and only starts working afterwards. In that case the delivered turn looks self-started, and the wake fires, because the brake only sees receipts at or after the arming tick. The cap bounds this to one wake per episode, so it can never loop. The pij-willing-coyote analysis on #52 has the details.

**Not done (follow-up).** Telling human-typed turns apart. The typed-turn hooks call `POST /v1/fyi/claim` on every submitted prompt, but nothing records a claim when no FYIs are pending. It is also unproven that a pij message typed into a pane doesn't trip the same hook. Until a human-only signal is proven, "no pij delivery since arming" is the proxy.

## Limit-reset wake

A fleet that went idle on a usage limit gets no edge nudge, because nothing can run. The edge round records `idle-on-limit` instead. The source is each Claude seat's last assistant transcript entry with `error: "rate_limit"`, `apiErrorStatus: 429` and `quotaLimits.resetsAt`, read from its final 64 KiB. The display text is never parsed.

- **A lifted limit is not a stop** (plan 178, #68). A rejection counts as a stop only until its wake is due, `resetsAt` + 120 s. A seat whose last record is still the rejection after that, because nobody restarted it, no longer pauses its scope. Until then, **one** stopped Claude seat pauses its whole scope's idle nudge and prime-active wake (ruling R1). There is no case of a missing `resetsAt`: a record without a numeric one is not read as a rejection at all.
- **One wake per `(scope, resetsAt)`**, at `resetsAt` + 120 s (`LIMIT_WAKE_MARGIN_SECS`). It is claimed in the store (`limit_wakes`) before it is sent, so restarts and overlapping rounds never wake twice. A reset already past when first seen wakes at once, and says so.
- **Target:** the lowest-id PA in scope. With no PA, the prime: the current designation if live in scope, else the lowest-id `prime`-role seat. This is the one fleet wake that can reach a prime.
- **Time is epoch only.** `resetsAt` is stored and compared as UTC epoch seconds. The wake shows the reset in the daemon's local zone, rendered at that instant, so a reset across a daylight-saving change shows the zone in force then (e.g. `03:00 AEDT`). Timezones never move the schedule.

## Presence and large seats (plan 217)

The idle nudge says nothing while workers are busy, and it is driven per PA, so a fleet whose PA is missing, or alive but asleep, heard nothing at all (6 Nov 2026: a PA took no turn for seven busy hours while its prime and 14 workers ran at 400k to 800k tokens). Four checks now ride the same 60 s round, per **scope**: a prime and the live seats in its repository, plus each PA's scope as above. A PA counts for the scope its parent's folder names; a directory `pa` holder is a `pa`-role seat, so it counts too. The thresholds are constants beside the trackers in `pij_core::watchdog` (Jordan, 2026-11-07).

- **Active.** A scope is active while its prime or one of its workers was observed `working` within the last 20 minutes (`FLEET_ACTIVE_WINDOW_MS`). Unknown states are never evidence, and a scope stopped on a usage limit is not active. A scope with fewer than two live workers (`PRESENCE_MIN_WORKERS`) never raises the PA alerts below.
- **PA missing → the prime.** An active scope with no live PA for 20 minutes (`PA_MISSING_AFTER_MS`) wakes its prime once: `[pij watchdog] <prime>: your fleet has been working for 20m with no live PA …`, naming the last PA with `pij-rs revive <pa>` (`--fresh` for a blank conversation), or `pij-rs spawn --harness omp --role pa --cwd <prime's folder>` when there was none. **What to do:** revive or spawn a PA, or decide the fleet goes without one. It re-arms when a live PA is seen in the scope, or after the scope has been seen inactive for a whole window.
- **PA dormant → the PA.** A live PA that has taken no turn for 30 minutes (`PA_DORMANT_AFTER_MS`) while its scope is active is woken once, with the fleet digest: `[pij watchdog] <pa>: your fleet is working but you have taken no turn for 30m …`. Its last turn is its latest `seat.activity` (turn start or end), or the last round that saw it `working`, whichever is later; with neither, the clock starts when this daemon first saw it. A PA observed `working` is never dormant. **What to do (PA):** run the round as for an idle nudge, then end the turn. It re-arms when the PA takes a turn.
- **PA unresponsive → the prime.** No PA turn 10 minutes after that wake (`PA_UNRESPONSIVE_AFTER_MS`) wakes the prime once: which PA, how long it has been silent, the dormant wake's message id and delivery receipt, and the choices. **What to do:** check its pane (a question dialog, a message parked in its composer, a wedge), revive it, or replace it. It re-arms with the dormant wake, when the PA takes a turn. With no prime in scope it is `braked` and fires when one appears.
- **Large seat → an FYI for the prime.** A seat in the prime's fleet (the live seats in its repository, its PA included, and live seats whose parent chain reaches one of them) whose context is over 600,000 tokens (`LARGE_SEAT_TOKENS`) is named to the prime in a **held FYI, never a wake**: it rides in on the prime's next turn. One line per seat: size, role, model, and the scope's PA. Crossings in one round share one FYI. It re-arms when the seat is seen at or under the line (compacted, or revived fresh). A seat with no known size never counts. **What to do:** usually nothing; it tells you what your PA should already be handling. With no PA, compact the seat or ask it for a handover.

**Once per stretch, across restarts.** Each message is claimed in `presence_claims` (migration 0038) before it is sent, keyed by check and subject (the scope, the PA, or the seat). A claim is released only on positive evidence (a live PA, a PA turn, a scope watched inactive for a whole window, a seat under the line) or when its PA or seat is no longer live. A restarted daemon starts its in-memory trackers afresh, which can only delay a message; the stored claim stops a repeat. A refused or failed delivery releases the claim, so the next round retries. A daemon that stops between the claim and the send loses that one message rather than repeating it.

**Copilot sizes.** Copilot writes its session size only in the ephemeral `session.usage_info` event, which never reaches `events.jsonl`, so the transcript reader reports Copilot's `contextUsedTokens` as unknown. The Copilot extension keeps the latest `currentTokens` and `tokenLimit` and sends them as `context_tokens` and `context_window_tokens` on the `/v1/activity` post it already makes at each turn boundary. The daemon fills an unknown fact from that report (basis `native`), for the same session only; a size the transcript knows always wins. `pij list`, the digest, the large-seat FYI and the cold-wake guard all see it. The report is held in memory, so after a daemon restart a Copilot seat's size is unknown until its next turn boundary.

## Cold wakes

Every PA wake goes through the ordinary delivery path, so the composer and human-typing gates still apply. None is held for a cold or long PA, because waking it is the point. Before sending, the round asks the send guard for its own verdict (`http::cold_wake::verdict`). When the PA is cold, the round appends `send.cold-wake-forced` with that wake's reason, exactly like a forced `pij send`. The receipt's label (`clear`, `busy`, `forced`, `unknown: …`) is recorded as `cold_check`.

## Audit kinds

| Event | When | Key fields |
|---|---|---|
| `watchdog.round` `{outcome: "nudged"}` | idle edge | `prime` (PA's parent), `scope`, `seats`, `delivery`, `cold_check`, `prime_active` |
| `watchdog.round` `{outcome: "idle-on-limit"}` | idle edge on a limit | `prime`, `scope`, `prime_active` |
| `watchdog.round` `{outcome: "prime-active"}` | every non-waiting prime-active verdict | `verdict` (`sent`, `braked`, `capped`, `idle-on-limit`), `reason: "prime-active"`, `prime` (the watched prime), `scope`; on `sent` also `seats`, `delivery`, `cold_check` |
| `send.cold-wake-forced` | any wake of a cold PA | `reason`, `context_tokens`, `idle_ms`, `estimate_usd` |
| `watchdog.limit-scheduled` | first sight of a `(scope, resetsAt)` | `due_at_ms`, `stopped` |
| `watchdog.limit-wake` | the reset wake | `target`, `target_source`, `late`, `delivery` |
| `watchdog.round` `{outcome: "pa-missing"}` | each change of the missing-PA verdict | `verdict` (`waiting`, `sent`, `capped`, `armed`, `braked`), `scope`, `prime`, `workers`, `missing_ms`; on `sent` also `last_pa`, `delivery`, `cold_check` |
| `watchdog.round` `{outcome: "pa-dormant"}` | each change of a PA's dormant verdict | `verdict`, `scope`, `pa`, `quiet_ms`, `reason`; on `sent` also `seats`, `delivery`, `cold_check` |
| `watchdog.round` `{outcome: "pa-unresponsive"}` | each change of a PA's unresponsive verdict | `verdict`, `scope`, `pa`, `prime`, `quiet_ms`; on `sent` also `wake` (the dormant wake's `msg_id`, `delivery`, `cold_check`), `delivery`, `cold_check` |
| `watchdog.round` `{outcome: "large-seat"}` | a seat crossing (`sent`) or re-arming (`armed`) | `prime`, `scope`; on `sent` also `tokens`, `msg_id`, `delivery` |

## Cost per round

Between events, a round reads no spine history: one registry list, one role join, and a size and mtime check per Claude transcript. The fleet digest (session reads with a 3 s wait, plus stale-card reads) is built only for a nudge or a prime-active `sent`. The brake's spine read happens only on a would-be wake.

The presence checks add no launch, timer or poll. Per round: the current prime designation and one `presence_claims` read; per live PA in an active scope (or holding a dormant claim), one indexed `LIMIT 1` read of its latest `seat.activity`; and per fleet seat that is working, was working last round, or has no size yet, one session-status read through the same bounded reader (an idle seat's context does not grow, so its last size stands). Repository keys come from the round's folder cache; a folder git cannot place is now remembered for 10 minutes (`UNPLACED_TTL`) instead of asked again every round. Audits are written only when a verdict changes.

## Opt-in watchdog (not a PA)

Any seat that is not a PA (a worker or a prime included) is watched only while it is opted in with `pij watchdog on`, and the nudge goes to that seat itself. It gets a stall nudge after a whole quiet interval (`PIJ_RS_WATCHDOG_SECS`, default 20 min), never while it is working or has declared a wait. See [pij-rs-api.md § Opt-in watchdog](pij-rs-api.md#opt-in-watchdog).

Two brakes keep it from costing more than it saves (plan 178). Each can only ever spare a nudge:

- **Cold seats are skipped.** Before a nudge the round asks the send guard for its verdict (`http::cold_wake::verdict`), as the PA wakes do. Unlike them, a cold opted-in seat is **not** woken: the round records `watchdog.nudge` with `delivery: "skipped: cold"`, `cold_check: "cold"` and the guard's facts under `cold`, and tries again an interval later. There is no override; a cold seat that must come back is its PA's or prime's call, through a forced `pij send`. A sent nudge records its `cold_check` too.
- **`ready` answers it.** The nudge tells an idle but available seat to run `pij report state ready`. A seat that is `ready` and has been nudged since it last worked is not nudged again until the round sees it `working`. The round looks once a minute, so a turn shorter than that can be missed; the seat then stays quiet, the conservative side.

Both nudges and the PA brief name the same card command, `pij report now "<did>" "<next>"`; a core test pins the brief to the nudge text.

## `pij watchdog status`

`{pas, optins}`: the live PAs (role `pa`, a registry row, not tombstoned) and the opt-ins of live seats. A retired seat keeps its role or opt-in row in the store, but it is not watched, so it is not listed.
