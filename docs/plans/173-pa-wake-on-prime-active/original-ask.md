# Plan 173: the PA resumes when the prime does

**Prime:** pij-far-jackal. **Repo:** AI-Substrate/pij-fleet. **Base:** main 19953b2, which has #52 merged.
**Background:** read scratchpad/172/prime-active-answer.md (pij-willing-coyote). It confirms the gap.
**Worktree:** `git -C ~/pi-hacking/pij worktree add ../fleet-173-prime-active -b feat/pa-wake-on-prime-active origin/main`. Edit by ABSOLUTE path only (#42).
**First:** load the pij skill and the builder skill, then run the implement verb. **SCOPE FREEZE:** build exactly this.

## Jordan's scenario (2026-10-14)
- Overnight the workers go idle, so the PA is nudged once, finishes, and goes idle.
- In the morning Jordan talks to the PRIME.
- The PA must start working again too and resume its plan. Today nothing wakes it: the prime isn't a worker (#52 F1), and IdleEdge only re-arms when a worker works.

## Rulings
R1. **Trigger.** Once per worker-idle episode, wake each PA in scope when the prime moves from idle to working. "The prime" is the designated prime, or the `prime`-role seat in scope.
R2. **Arming.** The trigger is armed at the first tick after the idle edge where the PA AND the prime are both observed idle.
R3. **Causality brake.** Skip the wake if a pij message was delivered to the prime after the arming tick: a `delivery.*` receipt with that recipient on the spine. A prime turn caused by a peer message, including the PA's own report, never fires the wake. This is a BRAKE (removing it only sends more), and the PR must say so in those words.
R4. **Cap.** At most one `prime-active` wake per PA per worker-idle episode. A new episode starts only on #52's existing worker re-arm.
R5. **Delivery.** Use the ordinary delivery path, so the composer and typing gates still apply. It is not held for a cold PA, because waking it is the point. Audit it like the idle nudge, with reason `prime-active`. The body says the prime is active again after the fleet went idle, so resume your plan, and it carries the fleet digest.
R6. **Limit stops.** If the scope is stopped on a usage limit, send no `prime-active` wake; the limit-reset wake owns that case.
R7. **Not now:** no human-typed detection. The FYI-claim hook isn't proven human-only. Note it as a follow-up.

## Proof
- **Red first, using a fake clock and registry:**
  - an overnight idle followed by a prime turn wakes the PA once;
  - a second prime turn in the same episode sends nothing;
  - a prime turn caused by a delivery to the prime after arming sends nothing;
  - a PA report that falls between ticks (the PA is never seen working) plus a delivery receipt sends nothing;
  - a usage-limit stop sends nothing;
  - a new worker episode re-arms it.
- Mutation: delete the brake and the delivery test must go red. Delete the cap and the second-turn test must go red.
- `harness checks`, stage by stage under C12. GitHub CI must be green. Open a PR (standing-approved). Don't merge. Reply to pij-far-jackal with the URL.

## Forbidden
Same as Plan 172:
- The prod daemon and `~/.pij-rs` are read-only. Tests use temp stores only. No live panes.
- The flow files.
- Main checkout edits.
- `git add -A`, `reset --hard`, `checkout .`, `stash`.
- Pushing `refs/notes/*`. Verify `git ls-remote origin 'refs/notes/*'` is empty after pushing.
- `flowspace3 daemon`.
- C12: `nice -n 19`, `cargo -j 4`, `timeout -k`. Stop above loadavg 40.

## Jordan's words (2026-10-14)
- "overnight the PA might stop cause fleet idle, then i will come in and talk to prime, the PA should also be workn to resume its work."
- "can't help but notice plans are not being done with /builder and are not in docs/how". Then, to plans living in docs/plans/<ord>-<slug>/ on main again (reversing e9b9e22): "yes do it."

## Amendment from pij-evident-dog (accepted)
The brake disarms the trigger. It does not consume the cap (AC3).
