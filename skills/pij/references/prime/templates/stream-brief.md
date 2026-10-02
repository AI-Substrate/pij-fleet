# Stream brief — s<ORD>-<SLUG>
**From**: <o-prime id> · **Date**: <ISO> · **Lifecycle**: provisional/adopted

## Structure tree

```text
human
└─ o-prime <id> · window o-prime
   ├─ this stream <id> · window s<ORD>-<SLUG>
   └─ sibling <id> · window <name> · <status>
```

## Work item

- **Plan folder**: `docs/plans/<ORD>-<SLUG>/`
- **Worktree**: `<absolute worktree path>`
- **Branch**: `<stream branch>`
- **Base**: `<approved base branch>` at `<base SHA>`
- **Spawn evidence**: `<descriptor cwd + tmux window/pane pointer>`
- **Landing**: `/builder 8 ship` to PR merge; teardown only after merge or explicit abandonment
- **Human ask, verbatim**: <quote or durable pointer>
- **Current flow state**: <pointer>
- **Prior art**: <paths only>
- **Cross-repo artifacts** (include whenever foreseeable): vendor VERBATIM into
  your plan folder + record sha256 in a PROVENANCE file BEFORE citing — source
  repos may be severance-ruled; a citation into one rots the day it goes dark.
  Never prepend headers to a verbatim vendor (breaks byte-identity).

## Descriptive fence

Canonical prose fence: `<government-root>/spine.md#fences--stream-id`; migrated facts stay store-authoritative.
Resolve `<government-root>` via [Governance branch — the rules](../rituals/bootstrap.md#governance-branch--the-rules); instantiate absolute paths.

- Expected touch set: <exact paths derived from planned actions>
- Scratch: `.harness/temp/<stream-id>/**`
- Hard exclusions: <government/CLI-only flow state/another worktree>
- Known separate-branch overlap: <paths + eventual reconciliation point>
- New worktree-local path: persist, tell the o-prime, continue (tell-not-ask;
  stop only at hard boundaries or convergence — global invariant 11).

## Orient stack

1. Invoke `/pij prime`; stream triage loads `<skill>/references/prime/orchestrator.md`.
2. Portable global orient: `<skill>/references/prime/orient-global.md`
3. Consuming repo local orient: `<government-root>/orient-local.md`
4. This item brief
5. Invoke `/thesis` through the host skill mechanism
6. Human preamble + preamble checkpoint
7. Protocol/ritual pages only on demand

## Assignment and reporting

- Assignment stays provisional until the human preamble and first report.
- A validated plan stops at `WAITING_FOR_BUILD_CONFIG`; no implementation begins
  until the human confirms the recorded coder/reviewer profile.
- Report at preamble, every phase checkpoint, and ship using
  `claim · artifacts[] · shas[] · gates[] · observations[] · open[]`.
- Close-out: <named PRD requirement rows advanced + receipt per row + named reconciliation owner>; report to the prime, never edit its register.
- All A2A messages this stream sends (reports, replies, worker packets) follow
  § C10 — Wire discipline (`<skill>/references/00-routing.md` § Shared conventions).
- Work confined to this verified worktree/branch is notify-only; a pushed baton
  grant is required only at convergence or shared mutable resources.
- Fleet packets inherit this fence and name a narrower task allowlist (task
  scope, not cross-worktree synchronization).
- Window/identity: `s<ORD>-<SLUG>` / `<pij id>` / role `stream-s<ORD>`; worker
  panes split inside this window and inherit the worktree cwd.
