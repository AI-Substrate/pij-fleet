# Partition by file ownership, not by issue

**This is the rule the whole pattern rests on.** Get it wrong and a fleet does not go faster —
it manufactures merge conflicts and then serialises anyway, at the worst possible moment.

## The rule

> **Each stream owns a set of files exclusively. No two streams may write the same file.**
> Group issues *to* that ownership; never let an issue's boundary decide a stream's boundary.

This is the same single-writer discipline the government already applies to `.harness/government/`
(o-prime only) and to resources under a baton. A worktree removes edit-time contention; it
does **nothing** for two branches that both rewrite the same function.

## Worked example — the 2026-08-08 wave

Six streams were cut from 40 open issues. The naive partition is one issue per seat. The
partition actually used groups by file:

| stream | issues | owns (exclusively) |
|---|---|---|
| install blocker | #118 | daemon bootstrap / pij-home creation |
| send-path integrity | #128, #132 | send dispatch, `core/message.ts` |
| capability surface | #102, #153 | `core/orchestration/pa-capability.ts`, whoami |
| liveness fields | #142, #155 | `core/state.ts`, `core/platform/types.ts` |
| watchdog verdicts | #161, #148 | `core/watchdog.ts`, `core/daemon/watchdog-manager.ts` |
| detectors go silent | #114, #141, #154, #156, #125 | `core/anomalies.ts` |

**Note what the last row costs.** Five issues went to one seat — not because they are one
piece of work, but because `core/anomalies.ts` is one file and **six open issues want it**.
Splitting them across PMs guarantees a conflict; the single-writer rule wins over the
even-workload instinct.

**Note also what got held back.** `#130` (should an instrument be scored on whether it ever
surfaced anything) is a design question, not a code change, so it does not belong in a
delivery stream at all.

## How to derive the partition

1. **List candidate issues.**
2. **For each, name the files it must write.** Read the issue's cited `file:line`; do not
   guess from the title.
3. **Build the file → issues map.** Any file wanted by issues you were going to split is a
   collision.
4. **Merge streams until every file has exactly one owner.**
5. **Check the leftovers.** A file wanted by many issues (`anomalies.ts` here) either becomes
   one stream, or the wave shrinks and it waits.
6. **Declare ownership in every brief**, explicitly, as a boundary the PM may not cross
   without asking.

## Partial collisions — name them, do not hide them

Two streams touching *distant regions of one large file* (`core/cli.ts` is 5k+ lines) will
usually merge cleanly, but "usually" is not a plan. Record it as a known risk and **sequence
those two merges** rather than landing them together.

In the 2026-08-08 wave: streams 2 and 3 both touch `cli.ts` — send dispatch vs the `whoami`
block. Low odds, non-zero, sequenced deliberately.

## Why this is not obvious

The instinct is to partition by **issue**, because issues are how the work is *described*, and
they look independent — they have different titles, different reporters, different symptoms.
Two issues can be conceptually unrelated and still be one merge conflict.

`#148` (the watchdog's `stalled` is an absorbing state) and `#161` (`responsive` is the
initialisation value) read as different bugs by different reporters. Both rewrite the verdict
path in `watchdog-manager.ts`. **Conceptual independence is not physical independence**, and
only the second one matters at merge time.

This is a specific instance of `.harness/government/doctrine/ask-what-else-satisfies-the-shape.md` — the
question *"what else touches this file?"* is exactly the *"what else satisfies this shape?"*
question, asked about a partition instead of a predicate.
