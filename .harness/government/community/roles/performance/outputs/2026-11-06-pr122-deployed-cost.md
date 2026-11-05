performance: pass-with-notes (head e4527770a69db16cf5b3a85df528d4e08c21672a)

**Directory/rename sites passed the feature-at-rest check, but overall cost was not unchanged:** total daemon rate was 1.149918/s versus the older control's 0.516582/s, and whole-machine rate was 774.0/s versus 177.9/s. This is not attribution of the increase to s04.

Prime-reported deployed merge f327a7dd579658be89dc2eb1aa890744dd95bb78, PID 92221. Measured **after both write batches**, 09:12:01.797 to 09:13:01.802 +10:00, after import 5a174e0 at 09:07:07 and last holder assign 46b38eb at 09:09:45. PM held directory writes out of the window. Boot age exceeded ten minutes.

| Metric | Historical 04 Nov control | Current 06 Nov window |
|---|---:|---:|
| PID in both pings | 97253 | 92221 |
| Actual interval | 60.009864125s | 60.004265875s |
| Spawn total | 494 -> 525 | 1303 -> 1372 |
| Daemon launches/s | 0.516582 | 1.149918 |
| Whole-machine 20s probe | 177.9/s | 774.0/s |

Current feature keys were present and flat: git.community 63 -> 63, ddocs.community 32 -> 32, tmux.rename-window 8 -> 8. All +0. The total +69 came from registration-native-process +60, date.utc-offset +7 and pa-watchdog-common-dir +2.

The comparison is the last recorded 60s at-rest window, **not an immediate pre-bounce PID 60418 sample**. Other deployments and different workload intervene. Do not claim a controlled no-regression result, subtract counters across restarts, or label all machine activity as the directory's. The whole-machine sampler classified only a lower bound of 1.1/s as daemon children; 599.4/s were gone before classification.

Portable receipt with complete counter pairs, timing/commit provenance, probe output and CPU/RSS: governance role memory `2026-11-06-plan195-at-rest.md`. The feature-at-rest launch property is supported; the wider-rate observation remains a separate follow-up.
