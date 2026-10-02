# Orient — local (lever 2)
**Scope**: THIS REPO · generated fresh · o-prime single writer
Resolve `<government-root>` via [Governance branch — the rules](../rituals/bootstrap.md#governance-branch--the-rules); substitute its absolute path in this generated orient.

## What this project is

<Human-approved product one-liner, users, and non-negotiable design pillars.
Read the real PRD; do not derive the project from source filenames alone.>

## Mandatory orient reads

These files do not reliably auto-load in every harness. Read them during
orientation/preamble:

- `<PRD / product thesis>`
- `<AGENTS.md or repository rules>`
- `<architecture / doctrine / domain map>`
- `<current portfolio and prior-plan pointers>`

## What matters here

- <determinism, compatibility, or data contracts>
- <configuration/tunability doctrine>
- <how behavior proves itself; known green-but-wrong failure modes>
- <where prior art and operator guides live>
- PRD reconciliation owner: <named current o-prime or explicitly assigned successor; transfer at rotation>

## Harness surface

| Need | Command | Evidence |
|---|---|---|
| Discover/boot | `<command>` | <readiness output> |
| Cheap proof | `<command>` | <artifact/path> |
| Full proof | `<command>` | <artifact/path> |
| Capture | `harness observe "<what>" --kind <kind>` | observe buffer |
| Encode | <check/generator/default path> | <committed substrate> |

## Repo mechanics — derive, do not copy

| Question | This repo's answer |
|---|---|
| Cheap quality gate | <command> |
| Full pre-ship gate | <command> |
| Notify-only worktree actions | <ordinary isolated reads/edits/gates/commits/sole-owner push> |
| Non-hermetic commands (write outside the worktree) | <audits, installs, links, global config — name them; command names are not proof of hermeticity> |
| Batons: what breaks under two concurrent users or converging histories? | <resource/convergence + free probe> |
| Never-stage list | <generated/local paths> |
| Flow-state rule | <CLI-only single-writer files> |
| Worktree root | <absolute parent directory for stream worktrees> |
| Worktree naming | <path pattern derived from stream id/slug> |
| Base branch | <approved default base + how to resolve its SHA> |
| Landing policy | `/builder 8 ship` → PR/CI/confirmed merge |
| Shared-tree fallback | <who may rule it + required index/commit discipline> |
| Fleet defaults | <harness/model/review route> |
| Human digest channel | <destination + self-id format> |
| Ceremony tier | <safe cheap worker/process> |

## Current portfolio context

- Portfolio: <authoritative store project; for unmigrated prose, `<government-root>/prime-flow.json`>
- <in-flight item, owner, important seam>
- <known hazard or sequencing watch>

## Authoring checklist

- [ ] Product one-liner and pillars confirmed by human/PRD
- [ ] Mandatory non-auto-loaded reads named
- [ ] Commands mechanically discovered, not guessed
- [ ] Notify-only worktree actions explicit; fences are not grants; batons cover
      only proven shared resources and convergence, with "free" probes
- [ ] Non-hermetic commands named — hermeticity derived, never assumed from names
- [ ] Never-stage and flow-writer rules explicit
- [ ] Worktree root/naming/base and Landing policy mechanically derived
- [ ] Shared-tree fallback is explicit, not the construction default
- [ ] Portfolio section reflects the live spine
- [ ] Portable lessons proposed upstream instead of copied into this repo
