# performance memory

- 2026-11-04 · The launch storm nobody's tests saw · cost, launches, history · green tests said nothing about 250–480 launches a second
- 2026-11-04 · Pre-community live-fleet baseline · baseline, cost, launches, cpu, rss · daemon 0.667 launches/s, machine 309.3/s; ping omits PID; fixed 60s schedule observed 60.002652s
- 2026-11-04 · Performance seat handover · handover, baseline, measurement-limits · nothing in flight; retain baseline timing, PID-provenance and attribution limits
- 2026-11-04 · PR 114 deployed cost · cost, deployment, cpu, memory, io · community counters stayed idle; preserve the large RSS/small footprint gap and 6.5G startup peak
- 2026-11-04 · Plan 197 BEFORE · spine, cpu, memory, io · one anomalies call: 13.8184s CPU and 6.4932 GiB sampled footprint; Mach CPU ticks require conversion
- 2026-11-04 · Plan 197 AFTER · spine, deployment, cpu, memory, io · fix confirmed: anomalies/node CPU down 13.4x/185.6x, footprints near 29 MiB, zero spawn deltas
- 2026-11-06 · Plan 195 at rest · deployment, launches, baseline · feature keys flat after both write batches; wider rates higher than the older unmatched Nov 4 control
- 2026-11-06 · Daemon spawn attribution · attribution, registration, utc-offset, cargo · date predates s04; conditional pending-seat tick path exists; original cargo share is unmeasured

<!-- pij:generated -->
- [2026-11-08 F112 after-reading and post-compact queue](2026-11-08-f112-after-reading-and-post-compact.dd.md): After compaction, use latest handover and await PM frozen heads (handover, cost)
- [2026-11-08 F112: preserve tap ownership before optimizing retries](2026-11-08-f112-preserve-tap-ownership-before.dd.md): When changing tap adoption rollback or investigating legacy launch storms (tmux, ownership, launches)
- [2026-11-08 Pin CI proof to the successful job artifact](2026-11-08-pin-ci-proof-to-the-successful-job.dd.md): When CI is green but JUnit fails, or retry logs and artifact counts disagree (ci, proof, artifacts)
- [2026-11-08 ROUTES timing: aggregate bound versus whole writes](2026-11-08-routes-timing-aggregate-bound-versus.dd.md): When evaluating ROUTES recomputation cost or scaling a governance directory (routes, cost, measurement)
<!-- /pij:generated -->
