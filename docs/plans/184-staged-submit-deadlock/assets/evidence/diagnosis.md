# Plan 184 diagnosis (ac-0001)

**Cause: process-launch latency against a 1 s stage bound.** The staged
`/compact` exceeded the adapter's 1 s stage bound (`SUBMIT_STAGE_LIMIT`,
`crates/tmux/src/lib.rs`) after the body was pasted and before Enter. The
commit-authorization check never ran, and Claude's slash-command menu played no
part: the transaction aborted inside `stage_submit`. The abort pasted
`STAGED_SUBMIT_RECOVERY` after the body and released the pane, leaving
`/compact` plus the notice in the composer. **This is not only a load
incident:** launch latency on this machine has a heavy tail at ordinary load
(evidence 6), and walrus failed the same way again at 08:03 today (evidence 7).

## Evidence

1. **Daemon log** (`~/.pij-rs/daemon.log`, read-only). These are the only two
   stage-bound failures in 2.46M lines. They match the two stuck panes:

   ```
   2026-10-28T08:46:09.415001Z pij-rs drain: tmux: staging for pane "%108" exceeded 1s; body was not submitted
   2026-10-28T09:29:35.46736Z pij-rs drain: tmux: staging for pane "%105" exceeded 1s; body was not submitted
   ```

   08:46:09Z is 18:46:09 local, the `deferral_since_ms` of walrus job 12791.

2. **Load at the time.** Both failures came before plan 182's launch trim went
   live: PR #84 merged at 19:54:06 local, and the daemon restarted at 19:54:51
   (`daemon.runtime.json` proc_start `20261028195451`). That daemon launched
   hundreds of processes a second under endpoint inspection (AGENTS.md
   § Resource cost). A short body costs three owned pastes, at two launches
   each, plus a read before and after. Today a tmux launch takes under 10 ms
   (`/usr/bin/time -p tmux -u display-message`, five runs, all `real 0.00`).

3. **Where the stall happened.** If the stall had come before the body, the
   in-loop bound would have aborted with no body pasted, leaving only the
   notice. Both composers hold the body, so the stall came at the closing
   paste or the `require_staged` read after it. The total also stayed under
   the 2 s watchdog, because the notice paste still held ownership.

4. **Reproduction on a private `-L` server.**
   `crates/tmux/tests/adapter.rs::slow_launches_strand_a_short_command_with_the_recovery_notice`
   slows only the third paste by 1.05 s and stages `/compact`. The adapter
   returns `staging … exceeded 1s`. The pane receives exactly
   `ESC[200~/compact ESC[201~ ESC[200~ \n <STAGED_SUBMIT_RECOVERY> ESC[201~`
   with no Enter, the bytes the stuck seats show. It passed in 3 of 3 runs.

5. **Live canary on main, normal load.** One Claude seat (haiku), spawned and
   then closed with its window killed:
   - idle, `pij send <canary> --command compact`: job 12880 `done`, and the
     composer cleared;
   - right after a turn and during a busy turn, with the compact queued while
     the seat worked: job 12881 `done`, and the pane showed
     `❯ /compact  ⎿ Compacted`.

   **Two single tries did not reproduce it.** With the tail measured in 6, a
   stage fails only sometimes, so two clean tries are expected. They do rule
   out the menu and the authorization check as a systematic cause.

6. **Launch latency today, at "normal" load** (load average 20, the daemon at
   200% CPU). 200–300 timed launches each, 20 ms apart, from a plain shell:

   ```
   true       p50=7.3 p90=140.0 p99=280.2 max=295.5   (ms)
   tmux -V    p50=12.4 p90=136.8 p99=294.9 max=296.1
   tmux query p50=9.1 p90=256.2 p99=390.0 max=553.4
   ```

   `/usr/bin/true` has the same tail, so the time goes into launching a
   process (endpoint inspection plus load), not into tmux or the pane's
   program. Nothing in a stage waits for the pane to read its input. Six
   launches drawn from this tail sum past 1 s often enough to explain the
   failures.

7. **Walrus again, 08:03 local today** (reported by the prime). The prime
   pressed Ctrl-C in both panes. Marmot's queued compact then committed. Walrus
   failed at once:

   ```
   2026-10-28T22:03:41.116019Z pij-rs drain: tmux: tmux: staging for pane "%108" exceeded 1s; body was not submitted; staged-submit recovery also failed: tmux: restore pane input and release ownership failed (exit 75): tmux returned no diagnostic
   ```

   **The `exit 75` is the 2 s watchdog.** The stage passed 1 s, and the abort
   spent two more launches pasting the notice. By the time the abort's restore
   ran, the watchdog script had already moved the owner from `<token>` to
   `<token>-recovering` and restored input itself. So `run_if_submit_token`
   found no matching token and ran `exit 75`. The notice had already landed,
   which is why the composer shows it. I found no evidence that session size
   matters: the walrus pane is 180×57 with `history_size` 0.

8. **The tail reproduced.** In
   `crates/tmux/tests/adapter.rs::staging_at_the_measured_launch_tail_still_submits`,
   every launch sleeps 0.35 s (the measured p99) before running tmux. Main
   fails it the same way walrus did:

   ```
   a stage at the launch tail must commit: … paste owned submit buffer failed (exit 75) … staged-submit recovery also failed: tmux: staged submit ownership for pane "%0" no longer matches token …
   ```

## Is the 1 s stage limit too tight? Yes; changed to 4 s, watchdog 2 s to 8 s

A one-chunk stage is six launches, and the abort adds three more (notice paste
and restore) before the watchdog may fire.

- At the worst observed launch latency (553 ms), six launches take 3.3 s, so
  the **stage bound is 4 s**.
- The **watchdog is 8 s**: more than the 4 s bound plus 1.7 s for the abort's
  three launches, so it can no longer take the token mid-abort.
- The bound only matters in the tail. A normal stage takes tens of
  milliseconds, and the human's input is disabled only for as long as a stage
  actually runs.
- Main fails the p99 test above; the branch passes it.
- Fewer launches per stage (one buffer for opener, body and closer) would cut
  both the cost and the tail. That is noted as a follow-up and not done here.

## Why it became a deadlock

The next attempt (`submit_with_owned_input`) reserved the pane (one launch)
and captured the composer. It found non-blank text, `/compact` plus the
notice, so it returned `Deferred { ComposerBusy, draft_sha }`. The worker
released the job with `Duration::ZERO` (`release_held_body`) and restored
input (a second launch). Nothing distinguished pij's own leftover from a human
draft. Nothing bounded the hold, and nothing told the sender. Delivery is FIFO
per seat, so every later message waited behind it. At the drain cadence of
about 5 s this came to 9,180 and 6,802 deferrals, two launches each.

Live job state (read-only sqlite, 2026-10-29 07:4x local):

| job | seat | command | deferrals | since |
|---|---|---|---|---|
| 12791 | walrus | compact | 9180 | 18:46:09 |
| 12836 | marmot | compact | 6802 | 22:14:26 (marmot's leftover dates from the 19:29 failure) |

Seven more rows wait behind walrus's head and two behind marmot's. Most are
repeated `compact` commands from the sender, who was never told.

## Cost reading before the fix (machine daemon, read-only)

```
2026-10-28T21:44:47Z {"total":46027,"r":16419,"x":16419}
2026-10-28T21:45:17Z {"total":46072,"r":16431,"x":16431}
```

`r` is `tmux.atomically reserve staged submit` and `x` is
`tmux.restore pane input and release ownership`. Each rose by 12 in 30 s, or
0.4/s, 0.8/s together, out of 1.5/s for the whole daemon.

## Side finding (out of scope, not fixed)

During the same window the log shows
`background reconciliation failed: background/refused: owner … is gone` at
about 10 Hz. Those lines make up much of the 500 MB `daemon.log`.
