# pair — a coder seat + a cross-model reviewer seat

> Route module — sibling-blind. Knows only this job; composition is the dispatch's job.
> Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: run one builder phase through two pij seats you parent: a **coder** that runs
`/builder 6 implement` and a separate **cross-model reviewer** that runs `/builder 7 review`.
You verify their receipts and own the merge. Both are ordinary peers spawned with the peer
route's native grammar. `/flow-pair …` resolves here; the former flow-pair CLI and its
`.flow-pair/` ledger are retired.

**Preconditions**: delivery ownership per § C1, `pij-rs ping --json` answers, a builder plan
with a tasked phase (`plan.dd.json`, `assets/tasks/phase-N/tasks.dd.json`), and a worktree and
branch for the work.

## Hard invariants

1. **Plan state has its own writers** — never hand-write `the-flow.json`/`the-flow.md`
   (dispatch invariant 1); builder records (`plan.dd.json`, `assets/**/*.dd.json`) change only
   through the builder skill's `ddocs` commands.
2. **Pointer delivery** — the packet is written to disk first; only its path is sent. Pointers,
   done-reports and verdicts follow § C10.
3. **Fence and forbidden paths in every packet** — the allowed write paths, plus the files in
   invariant 1 and any ledger dir as off-limits.
4. **The roster is the rs record** — your children in `pij list --parent <you> --json`, with
   the roles stamped at spawn. No side ledger.
5. **Different families** — the reviewer's model family differs from the coder's.

## Required PM report steps

Everything under `report` is a first-person claim about yourself. These are route steps:

```bash
pij report now 'Starting **<phase>**' 'Write the packet and dispatch the coder'      # before the first dispatch
pij report now 'Approved **<phase>** after `harness checks`' 'Send the [report](<path>)'   # only after the gates pass (Procedure 5), before the pointer
```

Inline markdown works; newlines are refused. `pij report question "<need>"` for a human
answer, `pij report blocked "<waiting on>"` for an external dependency. Working has no
state word; completion is `pij report state done`.

## Completion interrupt — compact EARLY

On a coder's done-report or a reviewer's verdict, compact that peer as the **first** tool
action (§ C3, fire-and-forget), then handle the report. Copilot seats compact like Claude
(plan 186); let it land before the next packet (§ C3). Paneless seats refuse `--command
compact`; leave them.

## Decision protocol

**You own the deliverable.** A coder's green tests and a reviewer's APPROVE are claims.
Trust them enough to keep moving, but cast your own eye over the load-bearing part before
recording approval.

| State | When | Action |
|-------|------|--------|
| `ASK_USER` | Requirement ambiguous / needs a human decision | Ask inline (dispatch invariants 9–10); block only dependent work |
| `RUN_LOCAL` | Safe, cheap, read-only | Do it yourself |
| `DELEGATE` | A tasked phase is ready | Write the coder packet → dispatch the pointer. **One whole phase per packet**, and say so |
| `REVIEW` | Coder reports done | Compact the coder → acquire/canary the reviewer if not live → dispatch the review packet at the exact head SHA |
| `FIX` | Verdict needs changes | Compact the reviewer → send the coder the review record's findings only |
| `APPROVE` | Verdict approves | Compact the reviewer → sanity pass → run the repo's gates → record approval and the stop report → merge or hand to the prime |

### Sanity pass — the last gate before APPROVE

One cheap glance, not a re-review:

- **Re-read the diff hunk** behind the highest-severity claim the reviewer cleared (or the one
  load-bearing guard). Does the code match the verdict's story?
- **Confirm Dim-0 ran** for code: the review carries mutation evidence (the guard, the change,
  RED→GREEN), not just "non-vacuous". No evidence = a missing proof → `FIX`.
- **Sniff for a rubber stamp**: an APPROVE with no findings, no files and no evidence on a
  non-trivial diff is suspect. Bounce it back or look yourself.

If the verdict doesn't survive, you do **not** record APPROVE.

## The seats

```bash
pij-rs spawn --harness <claude|copilot> --model <exact-model> [--effort <level>] --cwd <worktree> --role worker --name '<plan>-coder' --json
pij canary <seat> --expect-model <exact-model> --json
pij dispatch <seat> --packet <absolute-packet-path> --json
```

- **Models**: no defaults. Discover ids per § C4; pass effort with `--effort`, never a model
  suffix. OMP and Pi refuse a linked-worktree `--cwd` (peer route § Spawn and revive): for worktree
  work use `claude` or `copilot`, or spawn OMP/Pi from the main checkout and have it work by absolute path.
- **Lazy reviewer** — spawn the reviewer at the first `REVIEW`, never with the coder: an idle
  live peer still costs cache writes.
- **Canary before trust (§ C2)**, provided peers too. Place and name the panes (§ C5).
- **Reviewer never binds → HALT, never self-review.** If the reviewer cannot be spawned, its
  canary never passes, or `pij state <id> --json` shows it dead after one heal attempt, stop
  with a named error (`REVIEWER_UNAVAILABLE: <model> failed after N attempts`) to the human.
  Your sanity pass sits on top of an independent verdict; it never replaces one.
- **Reuse across phases** — compact, never close-and-respawn a healthy peer (§ C3).
- **Heal** a dead peer per ops § Recover after a reboot (revive resumes its conversation).
- **Teardown at the end** — close only seats you spawned; leave provided peers to their owner.

Builtin subagents are not peers here: a coder or reviewer is a real pij seat.

## Packets

**Coder packet** (one file): load the pij and builder skills and act from their contracts; run
`/builder 6 implement --plan "<plan.dd.json>"` (or follow the builder skill's
`references/stages/60-implement.md` by hand); worktree, branch, base SHA; the frozen scope and
fence; forbidden paths; git and machine rules (§ C11, § C12); what *done* means (ACs with
receipts, gates, PR); reply only with `pij send <you> --body-file <file>`.

**Reviewer packet**: the plan path, the exact head SHA, `/builder 7 review --plan "<path>"`,
Dim-0 mandatory for code (break the guard, rerun the targeted test, confirm RED, restore), the
review record under `assets/reviews/`, and the same reply rule.

## While a colleague is busy (§ C7)

After a dispatch, advance the next independent work. The daemon pushes the done-report, the
verdict, or a death obituary (§ C8); there is no stalled push. Never sit in a `pij state` loop
or message a seat that merely looks idle.

## Procedure

1. **Resolve intent** to a decision-protocol state.
2. **Write the coder packet**; `pij report now` (start); spawn/canary the coder if needed;
   dispatch.
3. **On done**: compact the coder; check its receipts (commit SHA, gate output, AC
   `proven_by`); write the reviewer packet at that SHA; dispatch.
4. **On the verdict**: compact the reviewer; sanity pass; `FIX` loops to step 3.
5. **Gate before approval** — on an APPROVE, run the repo's own gates (in pij, `harness checks`),
   all of them, not first-fail. A red gate is a `FIX`, not an approval.
6. **Record approval** in the plan, then the stop report and the completion pointer.
7. **Commit boundary** — § C11 on any commit you make.
