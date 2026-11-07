# AC4: live mid-turn steering canary (ruling C31)

Driver: [`steer-canary.mjs`](./steer-canary.mjs). Receipt: [`steer-canary-receipt.json`](./steer-canary-receipt.json) (run paths replaced by placeholders).

## Set-up

- A real Copilot CLI (1.0.108-1) running this tree's native pij extension, in its own pane.
- A private pij-rs daemon: a debug build of `source_commit` 6108221.
- A private tmux socket, `HOME` and `COPILOT_HOME`. Nothing touched the live fleet: the daemon log shows no foreign taps.
- A deterministic local model, not real inference, because no token was available. Each reply waits 10 s, and the model calls `true` through the shell tool for 8 rounds, so one turn a pij message starts lasts about 90 s.
- The seat is disposable: the daemon, tmux server and pane are all torn down at the end.

## Result: pass (exit 0)

Times are the last three digits of the unix second (jobs table: `enqueued_at`, `claimed_at`, `acked_at`).

### Run 1: steering on (the default)

| msg | queued | claimed | reached session |
|---|---|---|---|
| canary-start-1 (starts the turn) | 806 | 806 | 807 |
| canary-steer-1 | 813 | 813 | 817 |
| canary-steer-2 | 819 | 819 | 827 |
| canary-steer-3 | 825 | 827 | 837 |
| canary-steer-4 | 831 | 837 | 847 |
| canary-steer-5 | 837 | 847 | 857 |

- The model saw all five steered bodies inside the turn, in later requests of the same turn.
- The turn ended cleanly at 867 with the final reply, and the seat then read `idle`.

### Run 2: `pij steer off <seat>`, then one send during a second long turn

| msg | queued | claimed | reached session | turn ended |
|---|---|---|---|---|
| canary-wait | 874 | 947 | 947 | 947 |

- The message waited for the turn to end, and the model never saw it inside the turn.
- Its send receipt carried the turn-wait warning.
- The switch took effect at the next claim, with no restart.

## Finding: steers arrive one per model round

The extension claims the next message only after Copilot accepts the previous one, and Copilot accepts an immediate send at the next model round. So:

- each steer is held for at most one model round, plus the steers queued ahead of it;
- in run 1 that was 4 to 10 s per message, against up to a whole turn before C31;
- five steers queued in one round take five rounds to land.

That is the bound the PA flag (AC2) still watches. Batching a claim page into one injected turn would remove the queueing; it is a possible follow-up, not part of this plan.

The first attempt used 6 rounds, and steer 5 landed in the request after the final reply. That is still the same Copilot turn, and the seat still went idle. The check now uses 8 rounds so that every steer lands before the final reply, which keeps the assertion strict.

## Reproduce

```
nice -n 19 node docs/plans/214-held-delivery/assets/evidence/steer-canary.mjs --pij-bin <pij-rs built from this tree>
```

Copilot CLI 1.0.108 refuses `--no-remote`, so the driver doesn't pass it.
