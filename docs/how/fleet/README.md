# Fleet parallelism — many seats, many worktrees, one convergence

**Status**: living concept, evidence-collected, **destined to become a first-class `pij`
feature**. Nothing here is shipped tooling yet — it is the recipe we run by hand plus the
ledger of what it costs. `ledger.md` is the input to that feature.

## The concept

A **fleet** is N project-manager seats, each in **its own worktree on its own branch**, each
carrying a bounded slice of a larger body of work, running **autonomously** to a PR — with a
single o-prime holding the questions, the convergence order, and the human's attention.

```
                        Jordan (human)
                              ▲  questions, one at a time
                              │  1 sentence context + 1 sentence ask
                        ┌─────┴─────┐
                        │  o-prime  │  holds: partition · question relay · merge order
                        └─────┬─────┘  state: `question` while any ask is outstanding
        ┌───────────┬─────────┼─────────┬───────────┬───────────┐
      PM 1        PM 2      PM 3      PM 4        PM 5        PM 6
   worktree 1  worktree 2   …                                    each: /builder flow
   branch 1    branch 2                                          then /pij pair
        │           │                                            (coder + reviewer)
        └───────────┴──────────── PRs ─────────────────────────►  convergence (prime)
```

**The unit of parallelism is not the issue. It is the file.** See
[`partitioning.md`](./partitioning.md) — that is the load-bearing idea and the one most
easily got wrong.

## Why it is worth the overhead

Isolation removes **edit-time** serialisation. Six seats editing six worktrees never block
each other, never wait on a lock, and never see each other's half-finished trees. Convergence
is still serialised, and that is fine — merging is cheap when the branches are disjoint.

The overhead is real and should be stated: each seat needs a brief, a worktree, a spawn, and
supervision; each PR needs a review and a merge slot. Below roughly three streams the
coordination cost dominates and a single seat working sequentially is faster.

## When NOT to run a fleet

- **The work is not partitionable by file.** If every slice edits the same module, a fleet
  manufactures conflicts and you get one merge queue with extra steps.
- **The slices are dependent.** Stream B needing stream A's seam is a sequencing problem, not
  a parallelism opportunity. Order them, or give both to one seat.
- **The work is exploratory.** A fleet commits you to a partition before you know the shape.
  Explore first with one seat, then fan out.
- **Fewer than ~3 slices.** Coordination cost dominates.

## Documents

| file | what it is |
|---|---|
| [`partitioning.md`](./partitioning.md) | **the core rule** — partition by file ownership, not by issue |
| [`recipe.md`](./recipe.md) | the mechanical steps we actually run today, with the known traps |

## The invariants a fleet inherits

These come from `skills/pij/SKILL.md` and are restated here only because a fleet stresses
each one hard:

1. **Pointer delivery** — briefs are written to disk; `pij send` carries a path, never a body.
   (A fleet brief is far past the 280-char report limit, and a quoted body executes shell
   substitutions — pij#128.)
2. **Questions stay with their context owner** — a PM asks the human *through the prime*, in
   ordinary inline text. No modal question UI anywhere in orchestration.
3. **Isolation removes edit-time serialisation, not convergence-time serialisation** — work in
   a verified stream worktree is notify-only; synchronise at convergence.
4. **Everyone who supervises owes a status card**, including the prime, at its own altitude.
