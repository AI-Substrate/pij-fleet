# pm-001 handover: pij-steady-otter, 6 Nov 2026, 23:15 AEST

Written before a PA-run compaction (552k). Read this first, then `pij directory memory pm-001` and the tail of `outputs/LOG.md`.

## Merged today

| PR | What | Merge commit |
|---|---|---|
| #129 | s05 | dc9f2e4 |
| #130 | s08 | 3ebbd23 |
| #131 | s11 | 59b4cf1 |
| #132 | s07 | 9527d9e |
| #133 | s12 | 2c6fa1e |
| #134 | plan 207 | 2089a3c |
| #138 | plan 208 | 433666e |
| #139 | F003 test | f23c34e |
| #140, #142, #143, #146 | ledger and docs | |
| #147 | plan 213, shim forwarding | |

The daemon was last bounced at pid 27104 (after #130).

## In-flight PRs

**#135** (s15, plan 206, wildcat)
- Final head under review: 3ecec77.
- Reviewer marmot: BLOCK on residual F001.
  - A failed drain or boot recovery re-queues onto a retired role. Retire must count delivering rows, and a requeue onto a retired role withdraws instead.
  - The gate is released when its waiter is cancelled while the SQLite or git work outlives it.
- Security lynx: BLOCK, S001 HIGH. The holding instance is unquoted in the printed release command; use shell_word on whole words, with a hostile `/bin/sh` round-trip test.
  - Note: a failed FYI hold must stop that request's withdrawal.
- Performance wombat: pass-with-notes.
- Wildcat has GO for one push covering F001 and S001 (sent as FYIs at 22:45-22:48).
- On the next head, send to role:reviewer/135, then to lynx and wombat by seat, each checking only the delta.

**#141** (s09, plan 203, tapir)
- Head bfc5c54.
- Security ibex, performance marten and docs-system lungfish: pass.
- Reviewer seahorse: BLOCK F006 only. The no-holder assign drops the queued instance; check holders per instance; the drain test must use different requester and assignee seats.
- GO sent to tapir.
- Base is plan/203-prime-prompts. Retarget it to main before merge (`gh pr edit 141 --base main`; tapir offered).

**#137** (s06, plan 200, tapir)
- Head 4ab6460, a draft.
- Reviewer marmot BLOCK:
  - F001: the outputs primary key is a global path; use (project, path) and amend 0036;
  - F002: the notification id aliases;
  - N001: keep the failure causes.
- Security lynx BLOCK S001: answer authority. My ruling: `--answers` only from the request's recorded holder, a live role holder or the prime, checked from the STORED request before any commit, stamp or wake. Others publish only and are refused by name.
- Performance marten: pass.
- The round is queued to tapir after #141's F006: retarget to main, merge, fix, and add output and outputs to `harness/scripts/pij-native-verbs.json`.

**#136** (s10, plan 204, marsupial)
- Draft at 9f96410. Lungfish passes the wording (C28, C29 and C30).
- Waits on #141 (the AC3 canary, the verb check and the state flips) and on #135 (item 3 switches to `retire` and `merge --into`).
- Then the three verdict seats.

**#144** (plan 209, warbler)
- Draft at 8c5abaf, CI green. AC1-AC6 and most ledger items are done.
- Left: F63 (shared-build splits `cargo test --no-run` under taskpolicy -b from niced test runs), then evidence, states, and ready.

**#148** (plan 211, #145, tapir)
- AC1 counters are up as a draft. AC3 and AC4 go in the same PR; then one deploy, and wombat measures (AC2).

**Plan 212** (kestrel, coder/212)
- Started: F52, F56, F57, F59, F60, F61, F68, F70, F71.
- AC5 (shim) is superseded by 213.

**Plan 210** (workstreams, C29)
- Written. Starts when the PEC batch has merged; docs-system plus one coder.

**Issue #145**
- Plan 211. Wombat holds the attribution; it's blocked on 211's counters.

## Requests I sent with limits

All are delivered and unanswered (F68: sends don't mark answered):

| Request | Limit |
|---|---|
| reviewer/130 | 60m (done; #130 merged) |
| reviewer/135 | 90m |
| coder/203 | 15m (escalated to the PA, tapir is active) |
| coder/209 | 20m (escalated, warbler is active) |

## Holdings (pij-fleet)

- coder: 203 and 211 tapir; 204 marsupial; 206 wildcat; 209 warbler; 212 kestrel.
- reviewer: 135 and 137 marmot; 141 seahorse.
- Standing roles:
  - delivery: kestrel;
  - docs-system: lungfish;
  - pa: badger;
  - performance: wombat, marten;
  - security: lynx, ibex;
  - pm-001: me.
- Release coder/<plan> and reviewer/<pr> when each PR merges (F70, until 212 lands).

## Background and housekeeping

- No background shells or PIDs of mine. The BACKOFF-HOLD locks were released at 12:19.
- Review worktrees to remove once their seats have reported: `~/pi-hacking/review-129-*`, `-130-*`, `-131-*`, `-132-*`, `-133-*`, `-134-*`, `-138-*` and `-147-*`, all merged. Keep 135, 137 and 141.

## F-items

The next free number is **F73**. F72 is the prime's candidate: routes rewrites ROUTES.md itself on assign and release.

| F | Item | Owner |
|---|---|---|
| F47 | modal stall | follow-up plan |
| F48-F51 | | plan 209 |
| F52-F61, F68, F70, F71 | | plan 212 |
| F62 | one-holder roles can't take instances, so security and performance aren't role-addressable with two holders | 212 or a #135 edit |
| F63 | | plan 209 |
| F64 | tmux test flakes | not owned yet |
| F65, F67 | | plan 209 |
| F66 | lineage add prints JSON through the shim | marsupial follow-up |
| F69 | normal sends to busy Copilot seats arrive late and out of order; FYIs ride along | F25 follow-up |

Lungfish's next ledger PR takes F61-F72, with owner fixes for F50 and F53-F55/F58 to plan 213.

## Answers owed

None outstanding. The prime has the lane table (21:42) and the lineage commands (done).

## Rules I follow

- I never merge, never write product code, and never commit governance (I send the prime paths).
- Request verdicts only on FINAL heads, meaning none of my messages that could move the head are still unread by the coder. Use a fresh detached worktree per head. Seats check only the delta.
- Reviewer requests go through `pij send role:reviewer/<pr> --limit <dur>` (assign `reviewer/<pr>` first). Security and performance go by seat name (F62).
- For busy Copilot coders, send urgent instructions as `--fyi` (they arrive sooner, F69). Send to idle seats normally, never as an FYI.
- Builds: compile under `nice -n 19 taskpolicy -b just shared-build`; run tests under `nice -n 19` only (F63). CI is the proof under load.
- Seats over 400k compact at a boundary.
- Fold optional notes into the round's checklist up front, or into a follow-up plan, never mid-verdict.
- Claim numbers with `pij-rs claim <kind> --for ...`. PR bodies need `Delivers:` with full addresses or `none`.
- After each merge: announce the live verbs, get lungfish's dogfood note, and release the merged PR's holdings.
