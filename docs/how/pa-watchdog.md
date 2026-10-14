# The PA watchdog

The PA watchdog loop (`crates/daemon/src/pa_watchdog.rs`) runs every 60 s (`TICK`). Its fleet wakes go to **PAs only**, meaning seats whose asserted role is `pa`. A PA hears from it in the three cases below and no others. Two other paths in the same loop can reach other seats:

- the limit-reset wake goes to the scope's prime when the scope has no PA;
- an opted-in seat, worker or prime included, gets its own stall nudge ([below](#opt-in-watchdog-not-a-pa)).

No path wakes a worker because its fleet went idle.

| Wake | When | Sent from | Reason on a cold wake |
|---|---|---|---|
| **Idle nudge** (plan 172) | the fleet's workers go from someone working to no one working | `pij-bg` | `pa-watchdog: fleet idle` |
| **Prime-active wake** (plan 173) | after that, the prime starts again on its own | `pij-bg` | `prime-active` |
| **Limit-reset wake** (plan 172 R3) | a usage limit that stopped the fleet resets | `pij-bg` | `limit-reset` |

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
- **Usage limit:** while any Claude seat in scope is stopped on a quota rejection, the trigger sends nothing (`idle-on-limit`) and disarms, keeping the cap. The limit-reset wake owns that case.
- **Restart:** the tracker starts with no episode. A restart never sends a prime-active wake until a new worker episode opens.
- **Body:** `[pij watchdog] <prime> is active again after the fleet went idle: resume your plan.`, followed by the fleet digest.

**Known residual.** The prime's state is read once a minute, so one race slips past the brake. The PA's report, or any peer message, is delivered just before the arming round, but the prime is still observed idle at that round and only starts working afterwards. In that case the delivered turn looks self-started, and the wake fires, because the brake only sees receipts at or after the arming tick. The cap bounds this to one wake per episode, so it can never loop. The pij-willing-coyote analysis on #52 has the details.

**Not done (follow-up).** Telling human-typed turns apart. The typed-turn hooks call `POST /v1/fyi/claim` on every submitted prompt, but nothing records a claim when no FYIs are pending. It is also unproven that a pij message typed into a pane doesn't trip the same hook. Until a human-only signal is proven, "no pij delivery since arming" is the proxy.

## Limit-reset wake

A fleet that went idle on a usage limit gets no edge nudge, because nothing can run. The edge round records `idle-on-limit` instead. The source is each Claude seat's last assistant transcript entry with `error: "rate_limit"`, `apiErrorStatus: 429` and `quotaLimits.resetsAt`, read from its final 64 KiB. The display text is never parsed.

- **One wake per `(scope, resetsAt)`**, at `resetsAt` + 120 s (`LIMIT_WAKE_MARGIN_SECS`). It is claimed in the store (`limit_wakes`) before it is sent, so restarts and overlapping rounds never wake twice. A reset already past when first seen wakes at once, and says so.
- **Target:** the lowest-id PA in scope. With no PA, the prime: the current designation if live in scope, else the lowest-id `prime`-role seat. This is the one fleet wake that can reach a prime.
- **Time is epoch only.** `resetsAt` is stored and compared as UTC epoch seconds. The wake shows the reset in the daemon's local zone, rendered at that instant, so a reset across a daylight-saving change shows the zone in force then (e.g. `03:00 AEDT`). Timezones never move the schedule.

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

## Cost per round

Between events, a round reads no spine history: one registry list, one role join, and a size and mtime check per Claude transcript. The fleet digest (session reads with a 3 s wait, plus stale-card reads) is built only for a nudge or a prime-active `sent`. The brake's spine read happens only on a would-be wake.

## Opt-in watchdog (not a PA)

Any seat that is not a PA (a worker or a prime included) is watched only while it is opted in with `pij watchdog on`, and the nudge goes to that seat itself. It gets a stall nudge after a whole quiet interval (`PIJ_RS_WATCHDOG_SECS`, default 20 min), never while it is working or has declared a wait. See [pij-rs-api.md § Opt-in watchdog](pij-rs-api.md#opt-in-watchdog).
