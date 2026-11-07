# Plan 211 evidence: registration attempts (#145)

Shipped in #148 (merged at c3895da on 2026-11-07; daemon pid 28490). Verdicts at 03ba185: reviewer pij-double-bobcat pass-with-notes, security pij-bumpy-herring pass, performance pij-thorough-ocelot pass-with-notes.

## Before (issue #145)

About 0.64 registration `ps` launches a second at rest, with no record of which seats or paths paid them.

## After deploy (AC2, AC5)

Readings by performance (pij-thorough-ocelot), each two `pij-rs --json ping` snapshots 60 s apart, daemon pid 28490 at c3895da:

| Reading | Seats on the new extension | Registration attempts | Registration ps | Daemon launches |
|---|---|---|---|---|
| 1, 2026-11-07 ~20:19 | 6 of 9 (kestrel, wildcat, bison still on old code) | 0 | 0 | 0 in 60.0 s |
| 2, 2026-11-07 ~22:22 | 9 of 9 | 0 | 0 | 0 in 60.0 s |

Between the readings, only the three explicit reloads paid ancestry `ps` (4 in total), plus 2 zero-ps adopts. There was no growth under `unspecified` or `shim`. Receipts are in the governance branch: `roles/performance/outputs/2026-11/07-note-issue-145.dd.json` and `07-note-issue-145-2.dd.json`.

## What was not built, and why

- **tk-0004, the no-op repeat skip (AC3).** At rest, no healthy seat re-registers, so there is no repeat to skip. The cheap refusal for retired or other-session addresses did ship (#148, AC3's last sentence).
- **tk-0005, the shim's per-call registration (AC4).** The readings show no `shim` path attempts growing at rest. The Copilot client's stop on `E-RS-SEAT-RETIRED` did ship (#148).

Performance recommended deferring both on this evidence. Reopen them if a later reading shows registration `ps` at rest again.
