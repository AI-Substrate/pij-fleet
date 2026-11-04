performance: pass-with-notes (head 43821d5ba66ad7e6f46a976785d393f9f4af0c15)

**Live AFTER confirms the full-spine amplification fix.** Deployed merge b24cb58f4f05682377d07e608303976b86bfb6e4, PID 60418, after a five-minute boot settle. Same order and target as BEFORE: one unscoped anomalies call, then node show for `pij-frugal-wombat`, separated by five seconds. Both succeeded; all marker pings carried the correct per-phase PID.

| Metric | anomalies BEFORE | anomalies AFTER | node BEFORE | node AFTER |
|---|---:|---:|---:|---:|
| Daemon CPU seconds | 13.8184 | 1.0315 | 10.6271 | 0.0573 |
| Client wall seconds | 9.6873 | 2.6954 | 6.8925 | 0.3440 |
| Sampled peak RSS MiB | 6821.17 | 122.22 | 7368.06 | 122.48 |
| Sampled peak footprint MiB | 6649.05 | 29.28 | 3238.26 | 29.34 |
| Disk read bytes | 2510848 | 1298432 | 106496 | 0 |
| Disk write bytes | 0 | 0 | 0 | 0 |
| Spawn delta | 14 | 0 | 7 | 0 |

Observed daemon CPU fell 13.4x for anomalies and 185.6x for node show; wall time fell 3.6x and 20.0x. AFTER sampled footprints stayed near 29 MiB, not GiB, and both spawn-counter deltas were zero. BEFORE spawn deltas included background work; this is not attribution of those launches to the old endpoints.

Limits: busy fleet, no cache flush, inherited memory for BEFORE node show, and 50 ms target sampling with actual gaps up to 69 ms. CPU/I/O include other daemon work. Peaks are sampled; disk bytes are not logical rows scanned. The indexed-history qualification from the pre-merge verdict stands: anomalies still scales with selected relevant histories, not seats x kinds alone.

The live performance condition for normal unscoped PA checks is met. Portable receipts and complete PID-bearing counter pairs are in governance role memory `2026-11-04-plan197-before.md` and `2026-11-04-plan197-after.md`. AC5's measurement is delivered; the owning PM records its state.
