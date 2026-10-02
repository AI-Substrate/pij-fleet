# Government spine
**Thesis**: <human-authored purpose for the current portfolio>
**Updated**: <ISO from `date -u`> · **Seq**: <integer, +1 on EVERY write — answers "am I reading mid-write?" mechanically; a handover pack records the Seq it was written against>
**Writer**: <o-prime pij id; single writer>
**Prime-flow**: `<government-root>/prime-flow.json` (unmigrated prose only; store facts retain authority)
**Resolve** `<government-root>` via [Governance branch — the rules](../rituals/bootstrap.md#governance-branch--the-rules); instantiate absolute paths.
**PRD reconciliation owner**: <named current o-prime or explicitly assigned successor>

## Roster

| Stream | Plan | Worktree | Branch / Base | Window | Peer | Lifecycle/status | Fences | Batons held | Last report |
|---|---|---|---|---|---|---|---|---|---|

## Fences — <stream id>

> Descriptive ownership + merge-risk metadata; not an edit-time permission grant.

**Expected touch set**:
- `<exact write path>`
- `.harness/temp/<stream id>/**`

**Hard exclusions**:
- `<government/CLI-only flow state/another stream worktree>`

**Separate-branch overlap**: <none, or paths + affected streams; notification only>

**Convergence plan**: <reconciliation owner + baton/sequencing point for same branch,
moving dependency, rebase, landing, or merge>

## Sequencing watch

| ID | Trigger (verified, never hearsay) | Action | Status |
|---|---|---|---|

## Pending decisions

| ID | Owner | Question (verbatim) | Blocked nodes | Asked | Answered | Answer |
|---|---|---|---|---|---|---|

## Allocation ledger

- <ISO>: scanned existing ordinals; next free = <NNN>.
- <ISO>: reserved <ordinal>, `<folder>`, `<window>`, Worktree `<path>`, Branch
  `<branch>`, Base `<branch>@<SHA>`; status `preparing`.
- <ISO>: created/verified worktree before spawn.
- <ISO>: after PR merge or explicit abandonment, removed worktree and tombstoned
  <ordinal>, `<folder>`, `<window>`, and `<branch>`.

## Rulings

- <ISO> · <human/source> · <verbatim ruling or pointer> · <effects>
