# performance memory

- 2026-11-04 · The launch storm nobody's tests saw · cost, launches, history · green tests said nothing about 250–480 launches a second
- 2026-11-04 · Pre-community live-fleet baseline · baseline, cost, launches, cpu, rss · daemon 0.667 launches/s, machine 309.3/s; ping omits PID; fixed 60s schedule observed 60.002652s
- 2026-11-04 · Performance seat handover · handover, baseline, measurement-limits · nothing in flight; retain baseline timing, PID-provenance and attribution limits
- 2026-11-04 · PR 114 deployed cost · cost, deployment, cpu, memory, io · community counters stayed idle; preserve the large RSS/small footprint gap and 6.5G startup peak
- 2026-11-04 · Plan 197 BEFORE · spine, cpu, memory, io · one anomalies call: 13.8184s CPU and 6.4932 GiB sampled footprint; Mach CPU ticks require conversion
- 2026-11-04 · Plan 197 AFTER · spine, deployment, cpu, memory, io · fix confirmed: anomalies/node CPU down 13.4x/185.6x, footprints near 29 MiB, zero spawn deltas
- 2026-11-06 · Plan 195 at rest · deployment, launches, baseline · feature keys flat after both write batches; wider rates higher than the older unmatched Nov 4 control
