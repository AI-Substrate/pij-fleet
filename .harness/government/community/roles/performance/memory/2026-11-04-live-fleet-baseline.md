# Pre-community live-fleet baseline

tags: baseline, cost, launches, cpu, rss, req-19
hook: daemon 0.667 launches/s versus machine 309.3/s; ping omits PID, and the fixed 60s schedule observed 60.002652s.

## Context and limits

Requested by pij-far-jackal in message `17d8911e340d910b-000000000000074f`; measured by pij-community-perf, gpt-6.1-sol, before community implementation. Read-only fleet observation: no restart, bounce, deployment, build, install or pane input.

Main checkout HEAD was `79b535b0bb304709ff272f1118258c541179dcd3` before and after the measurements. Governance HEAD at entry was `10e351684ed108c41028e23ac14914f5eced77c2`. The live daemon reported `pij-rs 0.1.0`, healthy, online, healthy store with `timeout_ms: 2000`, and retired harnesses `["pi"]`. Ping does not expose a build commit, so checkout HEAD is context, not proof of the deployed daemon's source revision. Existing main-checkout changes in `.omp/models.yml` and `.agents/` were not touched.

Two limitations are explicit:

- The requested 60-second separation was scheduled against one monotonic anchor, not by sleeping after the first response. Actual invocation separation was **60.00265212508384 seconds**, 2.652 ms beyond the target due to scheduling. This is not a claim of mathematically exact 60.000000-second sampling.
- Both complete ping envelopes omit `data.pid`. Therefore the PID could not come from ping as requested. The existing spawn-rate probe independently found PID **97258** from the listening port; `/usr/sbin/lsof` then confirmed it listening on `127.0.0.1:7461`, and `/bin/ps` confirmed its command is `pij-rs daemon`.

## Daemon launch counters

Commands: `pij-rs --json ping`, piping each captured envelope through `jq .data.spawns`. Exactly two ping calls; both ping and jq exited 0. Full original envelopes are in the session receipt:

`<local session file>`

The receipt is a sequence of three JSON objects: sample 1, sample 2, interval. All per-site counter values are preserved below, so this note does not depend on retaining that session.

| Sample | UTC invocation start | Monotonic start, seconds | Ping duration, seconds | Total |
|---|---|---:|---:|---:|
| 1 | 2026-11-04T02:04:46.660957+00:00 | 776543.935046166 | 0.007801875 | 64801 |
| 2 | 2026-11-04T02:05:46.663742+00:00 | 776603.937698291 | 0.008707042 | 64841 |

Rates use the measured 60.00265212508384-second interval.

| Call site | Before | After | Delta | Launches/s |
|---|---:|---:|---:|---:|
| date.utc-offset | 20170 | 20184 | 14 | 0.233323 |
| git.pa-watchdog-common-dir | 2968 | 2968 | 0 | 0 |
| lsof.codex-process-files | 6 | 6 | 0 | 0 |
| ps.proc-liveness-fallback | 6 | 6 | 0 | 0 |
| ps.registration-native-process | 35248 | 35260 | 12 | 0.199991 |
| ps.sidecar-process-group | 1 | 1 | 0 | 0 |
| ps.tmux-harness-process-tree | 372 | 372 | 0 | 0 |
| ps.uds-peer-start | 5214 | 5228 | 14 | 0.233323 |
| sh.background-job | 15 | 15 | 0 | 0 |
| tmux.atomically reserve staged submit | 21 | 21 | 0 | 0 |
| tmux.authorize staged submit commit | 21 | 21 | 0 | 0 |
| tmux.clear-options tap owner | 35 | 35 | 0 | 0 |
| tmux.control-client | 1 | 1 | 0 | 0 |
| tmux.load submit buffer | 63 | 63 | 0 | 0 |
| tmux.new-window | 86 | 86 | 0 | 0 |
| tmux.paste owned submit buffer | 63 | 63 | 0 | 0 |
| tmux.pipe-pane attach | 192 | 192 | 0 | 0 |
| tmux.pipe-pane detach | 85 | 85 | 0 | 0 |
| tmux.restore pane input and release ownership | 21 | 21 | 0 | 0 |
| tmux.set-options tap owner | 192 | 192 | 0 | 0 |
| tmux.submit authorized staged body | 21 | 21 | 0 | 0 |
| **Total** | **64801** | **64841** | **40** | **0.666637** |

## Whole-machine 20-second probe

Ran from `/Users/jordanknight/pi-hacking/pij` as `/usr/bin/nice -n 19 just spawn-rate --seconds 20`, exit 0. Start 2026-11-04T02:04:46Z; end 2026-11-04T02:05:06Z. This window overlaps the first 20 seconds of the counter window.

```text
seconds: 20.0
daemon_pid: 97258
total_new_per_s: 309.3
polls: 249403
sampled_per_s: 305.5
attributed_per_s_lower_bound: {'<gone before seen>': 155.8, 'other': 149.2, 'pij-rs daemon': 0.5}
daemon_children_per_s_lower_bound: {'pij-rs': 0.5}
statusline_runs_seen_per_s: 0.0
top_commands_per_s_lower_bound: {'zsh': 78.8, 'bash': 30.1, 'node': 21.3, 'copilot': 7.6, 'pij-rs': 2.9, 'flowspace3': 1.6, '2.1.318': 1.5, '2.1.317': 1.3, 'git-ai': 1.3, 'sh': 1.0, 'git': 0.8, '2.1.312': 0.5}
```

`total_new_per_s` comes from PID-counter movement; attribution is a lower bound from libproc sampling, not an exhaustive accounting. The 309.3/s machine rate is high but is not evidence that the daemon launched 309.3/s. The zero sampled status-line rate does not prove no status-line processes ran. The probe itself uses an unpaced in-process poll loop, was niced, and performed 249403 polls; this is an observed busy-fleet baseline with observer overhead, not an idle-machine benchmark or a per-seat scaling estimate.

## Daemon CPU and RSS

At 2026-11-04T02:06:13Z, after the launch probes:

```text
$ /bin/ps -p 97258 -o pid=,pcpu=,rss=,time=,etime=,lstart=,command=
97258   9.1 121376 396:57.32 02-00:32:34 Mon  2 Nov 11:33:39 2026     /Users/jordanknight/.npm-global/bin/pij-rs daemon

$ /usr/sbin/lsof -nP -a -p 97258 -iTCP:7461 -sTCP:LISTEN
COMMAND   PID         USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME
pij-rs  97258 jordanknight   19u  IPv4 0x3fd4ae76fd3b9985      0t0  TCP 127.0.0.1:7461 (LISTEN)
```

Both exited 0. `/bin/ps` reported **9.1% CPU** and **121376 KiB RSS (118.53 MiB)**. CPU is the ps-reported percentage, not a CPU-time delta over the 60-second counter window. The daemon start predates both samples and counters increased monotonically; nothing was restarted.

For comparison after community code lands, retain the same windows and probe, record fleet workload and deployed revision when available, and distinguish daemon per-site launches from whole-machine launches. This baseline does not supply a cost verdict on code not yet reviewed.
