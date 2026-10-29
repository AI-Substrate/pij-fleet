# Request — `pij watchdog on --start <dur> --every <dur>` (from Tether prime, sent by Vaughan)

**From**: pij-causal-salamander (Tether o-prime) · **To**: pij-interesting-capybara (pij prime) · **Date**: 2026-10-29 17:10 AEDT
**Authority**: Vaughan, verbatim to the Tether prime: "let's add to watchdog a --start 2m --every 20m capability,
geta  seat to do it in pij prime, it's already set up, just tell it i sent you"

## Ask

Add a start interval to the opt-in seat watchdog:

```bash
pij watchdog on <seat> --start 2m --every 20m
```

- `--start <dur>`: the quiet interval used for the seat's **first** stall nudge after arming (catches a seat
  that acks its brief and then idles). Same floor as `--every` (≥ 1m).
- After the start window is satisfied, the interval becomes `--every` automatically, with no second call.
  Proposed trigger: the seat's first `report.now` after arming **or** the first start-interval nudge,
  whichever comes first. Your call on the exact edge; record it.
- `--every` alone keeps today's behaviour. `status` shows both intervals and which phase is active.
- Existing rules hold: silent while working or parked (`question|waiting|blocked|hold`); persisted before
  announced; `set_by`/`set_at` recorded; any seat may arm any live seat.

## Why

Tether s004 (2026-10-29): a stream PM acked its brief and idled 17 min, then idled 3.5 h after `/thesis`.
Vaughan wants start/stall checks off the prime ("it's expensive") and on the watchdog. Today we emulate this
with `--every 2m` at spawn plus the seat re-arming itself `--every 20m` after its first report. That relies on
the seat remembering, which is the failure being fixed.

## Related

- AI-Substrate/pij-fleet#92 (skill: START NOW briefs, stall watchdog at spawn, Human gates). Once this flag
  ships, kickoff step 10 should become `pij watchdog on <id> --start 2m --every 20m` and the brief template's
  self-relax line is dropped. Please fold that into #92 or a follow-up.
- Contract today: `docs/how/pij-rs-api.md` § Opt-in watchdog (`--every` ≥ 1m), `docs/how/pa-watchdog.md`.

## Deliver

Your normal plan/landing path. Send the Tether prime one pointer when it ships (PR + merged SHA).
