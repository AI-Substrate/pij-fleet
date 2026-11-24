# Workshop 003, input 003: the fate of every existing how-to page

**Date**: 2026-11-24. **By**: pij-intense-herring. **Feeds**: PRD 005 req-31 (no two layouts) and req-25.

Every page under `docs/how/` and `skills/pij/references/community/` gets one fate: it **becomes a procedure**, it
**stays as reference** (explains how something works; no numbered steps to run), or it is **deleted**. From this PR on,
`just procedures-check` fails on a page there with three or more numbered items that each begin with a command, unless the
page says why it is reference (`<!-- not-a-procedure: <reason> -->`). On 2026-11-24 only one page tripped it:
`change-seat-effort.md`, which becomes PROC-fleet-0001 in this PR.

## docs/how

| Page | Fate | Why |
|---|---|---|
| agent-feedback.md | becomes a procedure, later (req-32) | Its "Curator path" (triage the wish, triage difficulties, …) is a repeatable job; the rest is reference. |
| build.md | stays as reference | Describes the recipe surface and the gates; the runnable parts are `just install` and `harness checks`. |
| cache-proxy.md | stays as reference | How the sidecar works; "Use it" is two commands, not a procedure. |
| change-seat-effort.md | **becomes a procedure, in this PR** | PROC-fleet-0001; the page is removed. |
| claude-auto-seat.md | stays as reference | Behaviour of automatic Claude seats. |
| claude-statusline.md | stays as reference | One setting. |
| computed-but-unconsulted-signals.md | stays as reference | A failure pattern, with instances. |
| copilot-native-extension.md | stays as reference | The extension's contracts. Its "On a Copilot update" section is what PROC-release-0001 runs; it keeps one line pointing there. |
| daemon-cost.md | stays as reference | Cost model and measurements. |
| double-seat.md | stays as reference | A failure and its fix. |
| flow-pair.md | stays as reference | The flow-pair model; the skill drives it. |
| green-that-lies.md | stays as reference | Review patterns. |
| macos-defender.md | stays as reference | The Defender exclusion and how to measure launches. |
| pa-watchdog.md | stays as reference | Watchdog behaviour. |
| pij-federation.md | becomes a procedure, later (req-32) | "Setup on two machines" and "Key rotation and revocation" are repeatable jobs. |
| pij-orchestration-baton.md | stays as reference | Command and store reference. |
| pij-pane-signals.md | stays as reference | Delivery contract. |
| pij-platform.md | stays as reference | Authority and boundaries. |
| pij-prime.md | stays as reference | Governance index. |
| pij-rs-api.md | stays as reference | The API and refusal contracts. |
| pij-telegram.md | stays as reference | The bridge's behaviour. |
| skills.md | stays as reference | The skills model. |
| workflow.md | stays as reference | Its numbered sections name layers, not steps. |
| fleet/README.md | stays as reference | Why and when to run a fleet. |
| fleet/partitioning.md | stays as reference | The partition rule. |
| fleet/recipe.md | becomes a procedure, later (req-32) | "The fleet recipe" is a numbered run of steps (partition, streams, briefs, spawn, supervise, converge, close out). |

## skills/pij/references/community

| Page | Fate | Why |
|---|---|---|
| README.md | stays as reference | The rules for project docs, shared by every procedure below. |
| prd.md | **becomes a procedure, in this PR** | PROC-docs-0001 write a PRD; the page points there (req-25). |
| blueprint.md | **becomes a procedure, in this PR** | PROC-docs-0002 write a blueprint. |
| workstream.md | **becomes a procedure, in this PR** | PROC-docs-0003 write a workstream; the page keeps its reference sections (lanes, needs, shaping). |
| fold-a-ruling.md | **becomes a procedure, in this PR** | PROC-docs-0004 fold a ruling in. |
| roadmap.md | stays as reference | A two-line pointer kept so old links land. |
| examples/ | stays as reference | Worked example documents. |

Nothing is deleted outright: every page either still explains something or now points at its procedure.
