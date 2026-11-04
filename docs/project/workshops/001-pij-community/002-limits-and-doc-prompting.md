# Workshop 002: time limits, and prompting for project docs

Status: agreed, 2026-11-04. Jordan and the pij prime (`pij-far-jackal`). It answers the questions workshop 001 left open. PRD rows: req-22 to req-25 in `docs/project/prds/001-pij-community.dd.json`.

## Time limits are stall checks, not policing

The trigger was a one-off transcriber in a design session. It ran 75 minutes and made 661 tool calls with nothing on disk, and nobody could see its progress. It was busy, not quiet, so the existing watchdog (which only nudges quiet, opted-in seats) would never have fired.

1. A send can carry `--limit <duration>`. Without one, the role's default limit applies, for example 20 minutes for a transcriber.
2. When the limit passes with no output and no card update, pij sends the seat **one** check-in: write what you have, update your card with `pij report now`, and say why it needs longer. Nothing is stopped.
3. **Every card update restarts the limit.** A seat that keeps reporting progress is never checked in on. If it's working, all good.
4. A seat that does not answer the check-in is probably stalled, so pij tells the PA (or the prime when no PA is live).
5. **Cost:** the check runs inside the watchdog round the daemon already has, every 60 s. It adds no timer and no launches. The opt-in quiet-seat watchdog is unchanged.

## Prompting for PRDs, blueprints and roadmaps

6. The pij skill ships a short **procedure and a worked example** for each document type, and for folding a ruling into them.
7. **Who writes them:** by default the PM that owns the work, with the prime helping as needed. The prime writes them itself when Jordan asks, especially early in a project.
8. **Roadmap steps name the roles they need** (`needs: role:devcontainer`). The prime's view flags a needed role that does not exist. The prime offers to create it, from a template or as a new definition worked out with Jordan.
9. **pij's own PRD register** moves from `<government-root>/prd/` into `docs/project/prds/`, as a separate step after the first PR merges.
10. **The skill route is `/pij community`.** The commands stay `pij directory …`.
