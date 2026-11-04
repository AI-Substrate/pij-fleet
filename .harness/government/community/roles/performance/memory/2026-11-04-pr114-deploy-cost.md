# PR 114 deployed cost: flat community counters, RSS/footprint gap

tags: cost, deployment, launches, cpu, memory, io, measurement-limits
hook: community launch sites stayed idle; large resident memory and a small charged footprint must not be conflated.

## Result and provenance

Post-deploy cost verdict: **pass-with-notes** for PR #114 head `4ec59a851d224b0a7736c6366ee9f05684f8b447`, squash merge `6ccd4d8650fb5c41fcd747013e35819ed7aba8bb`. No counted community launch occurred in the AFTER window. CPU and resident-memory differences are observations, not proof of a PR-specific regression or of unchanged overall cost.

The prime requested BEFORE in message `17d8911e340d910b-00000000000009d4` and reported the deployment in `839ec87a6f134c8b-0000000000000000`. The BEFORE checkout was `4b14b211c8b10a39b597292a4cb101024db268c0`; the AFTER checkout matched the prime's reported deployed merge. Checkout identity alone is not a binary-source fingerprint.

Both runs were read-only. Same commands: two `pij-rs --json ping` calls targeting a fixed 60-second monotonic interval; `nice -n 19 just spawn-rate --seconds 20`; `/bin/ps` CPU-time/RSS samples and listening-port checks. The benchmark was capped at 40 seconds and stopped if load exceeded 40; observed maximum one-minute loads were 37.049 BEFORE and 28.564 AFTER. No daemon lifecycle action, directory write, install or build was performed by this seat.

AFTER PID 97253 started at 14:57:04.279 local time according to vmmap. The first ping at 15:02:05.100 was more than 300 seconds later. Each window had matching process start stamps and listener identity; AFTER ping also returned `data.pid: 97253`. BEFORE ping omitted PID, so its PID 97258 came from ps and the listener, not ping.

## Paired measurements

| Metric | BEFORE | AFTER |
|---|---:|---:|
| PID | 97258 | 97253 |
| First ping UTC | 2026-11-04T04:53:31.426005Z | 2026-11-04T05:02:05.100121Z |
| Second ping UTC | 2026-11-04T04:54:31.432102Z | 2026-11-04T05:03:05.110121Z |
| Actual ping interval, seconds | 60.005980541 | 60.009864125 |
| Scheduling overshoot, milliseconds | 5.981 | 9.864 |
| Spawn total, first -> second | 70549 -> 70570 | 494 -> 525 |
| Launch delta / second | 21 / 0.349965 | 31 / 0.516582 |
| Whole-machine launches / second, 20s probe | 208.4 | 177.9 |
| CPU time, first -> second | 426:16.95 -> 426:26.22 | 1:19.24 -> 1:30.56 |
| CPU-time delta, seconds | 9.27 | 11.32 |
| CPU sample interval, seconds | 60.05626875 | 60.04982575 |
| Average CPU, percent of one CPU | 15.4355 | 18.8510 |
| RSS, first -> second, KiB | 101632 -> 101344 | 4898384 -> 4553312 |
| Probe exit code | 0 | 0 |

CPU increased by 3.4155 percentage points in these two busy-fleet windows. They did not hold workload constant. Whole-machine rate fell while daemon rate rose; neither is attribution of the change to community code.

These are two independent within-incarnation deltas. Raw cumulative counters across the restart must not be subtracted.

## Complete per-site counter pairs

`-` means the key was not reported, not an observed numeric zero. The earlier private-daemon write receipt produced positive `git.community` and `ddocs.community` keys; neither key appeared in either AFTER snapshot.

| Site | BEFORE first | BEFORE second | AFTER first | AFTER second |
|---|---:|---:|---:|---:|
| date.utc-offset | 22040 | 22047 | 59 | 68 |
| git.pa-watchdog-common-dir | 3313 | 3315 | 53 | 53 |
| lsof.codex-process-files | 6 | 6 | - | - |
| ps.proc-liveness-fallback | 7 | 7 | - | - |
| ps.registration-native-process | 37324 | 37336 | 105 | 117 |
| ps.sidecar-process-group | 1 | 1 | 1 | 1 |
| ps.tmux-harness-process-tree | 422 | 422 | 4 | 4 |
| ps.uds-peer-start | 6476 | 6476 | 48 | 58 |
| sh.background-job | 16 | 16 | 1 | 1 |
| tmux.atomically reserve staged submit | 24 | 24 | - | - |
| tmux.authorize staged submit commit | 24 | 24 | - | - |
| tmux.clear-options tap owner | 43 | 43 | - | - |
| tmux.control-client | 1 | 1 | 1 | 1 |
| tmux.load submit buffer | 72 | 72 | - | - |
| tmux.new-window | 108 | 108 | - | - |
| tmux.paste owned submit buffer | 72 | 72 | - | - |
| tmux.pipe-pane attach | 221 | 221 | 111 | 111 |
| tmux.pipe-pane detach | 110 | 110 | - | - |
| tmux.restore pane input and release ownership | 24 | 24 | - | - |
| tmux.set-options tap owner | 221 | 221 | 111 | 111 |
| tmux.submit authorized staged body | 24 | 24 | - | - |
| git.community | - | - | - | - |
| ddocs.community | - | - | - | - |

The BEFORE delta was date +7, watchdog git +2 and registration ps +12. The AFTER delta was date +9, registration ps +12 and UDS-peer ps +10. All other reported deltas were zero. Counter sums, differences and PID continuity were checked programmatically.

## Whole-machine probe receipts

BEFORE:

```text
seconds: 20.0
daemon_pid: 97258
total_new_per_s: 208.4
polls: 237383
sampled_per_s: 204.3
attributed_per_s_lower_bound: {'other': 102.2, '<gone before seen>': 101.6, 'pij-rs daemon': 0.5}
daemon_children_per_s_lower_bound: {'pij-rs': 0.5}
statusline_runs_seen_per_s: 0.0
top_commands_per_s_lower_bound: {'bash': 47.5, 'node': 37.7, 'copilot': 2.7, 'flowspace3': 2.3, 'zsh': 2.3, 'sh': 1.7, 'git-ai': 1.4, 'chrome-headless': 1.2, '2.1.312': 1.1, 'pij-rs': 1.0, '2.1.318': 0.9, '2.1.317': 0.7}
```

AFTER:

```text
seconds: 20.0
daemon_pid: 97253
total_new_per_s: 177.9
polls: 220512
sampled_per_s: 172.1
attributed_per_s_lower_bound: {'<gone before seen>': 92.8, 'other': 78.9, 'pij-rs daemon': 0.4}
daemon_children_per_s_lower_bound: {'pij-rs': 0.3, 'date': 0.0}
statusline_runs_seen_per_s: 0.1
top_commands_per_s_lower_bound: {'bash': 33.5, 'node': 28.7, 'copilot': 5.1, 'flowspace3': 2.8, 'pij-rs': 2.3, 'sh': 1.4, '2.1.318': 1.3, 'git-ai': 0.9, 'chrome-headless': 0.8, 'turbo': 0.6, 'zsh': 0.5, 'git': 0.5}
```

The probe is an unpaced, niced libproc polling loop. Attribution is a lower bound, and the observation itself has overhead.

## Memory and I/O finding

**RSS/physical-footprint gap, not an established private-memory regression:** AFTER RSS fell from 4.6715 to 4.3424 GiB during the measured minute, versus BEFORE ending at 98.96875 MiB. `proc_pid_rusage` independently agreed exactly with both AFTER ps RSS values. Its charged physical footprint was much smaller, 94.9270 -> 95.3645 MiB. These metrics must not be substituted for one another.

The installed SDK's `rusage_info_v2` layout and `proc_pid_rusage` contract were checked before calling it. Both calls succeeded, returned a positive process start identifier and residency, and reported the same incarnation.

| AFTER resource counter | First | Second | Delta |
|---|---:|---:|---:|
| Resident bytes | 5015945216 | 4662591488 | -353353728 |
| Physical footprint bytes | 99538128 | 99996880 | +458752 |
| Disk read bytes | 2261446656 | 2261581824 | +135168 |
| Disk write bytes | 9097216 | 9097216 | 0 |
| Pageins | 1 | 1 | 0 |

Resource-counter separation was 60.030180584 seconds. That is 132 KiB read and zero bytes written during the AFTER window. The cumulative read count was already about 2.26 GB at five minutes; it is not the measured-minute read delta. No matching BEFORE resource-usage sample exists, so there is no before/after I/O comparison.

A supplementary `/usr/bin/vmmap -summary 97253` at 15:03:25.797, outside the timed window, returned these raw lines:

```text
Physical footprint:         95.4M
Physical footprint (peak):  6.5G
                                VIRTUAL RESIDENT    DIRTY  SWAPPED VOLATILE   NONVOL    EMPTY   REGION
REGION TYPE                        SIZE     SIZE     SIZE     SIZE     SIZE     SIZE     SIZE    COUNT (non-coalesced)
Malloc Small                     652.0M   467.9M    64.1M     400K       0K       0K       0K      165
Malloc Small (empty)               5.3G     3.8G    10.3M      96K       0K       0K       0K     1365
TOTAL                            393.2G     4.6G   111.8M    1600K       0K      16K       0K     3607
```

```text
                                 VIRTUAL   RESIDENT      DIRTY    SWAPPED ALLOCATION      BYTES DIRTY+SWAP          REGION
MALLOC ZONE                         SIZE       SIZE       SIZE       SIZE      COUNT  ALLOCATED  FRAG SIZE  % FRAG   COUNT
DefaultMallocZone_0x1066ec000       6.0G       4.3G      98.9M       656K      71117      13.7M      85.9M     87%    1582
```

This independently corroborates the small current footprint, the 6.5G peak and a large resident allocation marked empty. It does not by itself establish the cause or future reclaim behavior. The prime should retain the startup peak/RSS observation as a follow-up, not silently report memory unchanged or call RSS charged heap.

Disk-byte counters do not count cached lstat/open operations, identify filesystem call frequency, or attribute I/O to the new checks. No production directory write was induced; this window does not measure per-write metadata overhead. The earlier private-daemon success costs remain 7 git + 2N ddocs, with bounded retries separately.
