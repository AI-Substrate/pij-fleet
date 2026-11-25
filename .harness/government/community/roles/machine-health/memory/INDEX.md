# machine-health memory

<!-- pij:generated -->
- [2026-11-25 2026-11-25 baseline & cross-repo recurring causes](2026-11-25-2026-11-25-baseline-cross-repo-recurring.dd.md): Six recurring Mac health causes across all repos; disk at 21 GiB free \(\<25 GiB floor\)
- [2026-11-25 2026-11-25 git-ai metrics-db deleted \(+23.1 GiB\) & root cause](2026-11-25-2026-11-25-git-ai-metrics-db-deleted-23.dd.md): Freed 23.1 GiB \(27.2 -\> 50.3 GiB free\); git-ai respawned in 2s and writes Copilot events with no api\_key flush
- [2026-11-25 2026-11-25 git-ai respawn watch after hook removal](2026-11-25-2026-11-25-git-ai-respawn-watch-after.dd.md): git-ai bg respawned in 5s \(PID 13804, +11.8MB/15m\) from pre-14:02 Copilot seats \(ostrich %1093, flamingo %1094, 74 live seats\)
- [2026-11-25 2026-11-25 orphan sleep cleanup & pr-freeze prune](2026-11-25-2026-11-25-orphan-sleep-cleanup-pr.dd.md): Stopped 11 verified crates/tmux orphan sleep PIDs \(0 remaining\) and pruned review-280 + review-292 via just pr-freeze --prune
- [2026-11-25 2026-11-25 prime ruling: git-ai fade-out + 2026-11-26 check](2026-11-25-2026-11-25-prime-ruling-git-ai-fade-out.dd.md): PENDING 2026-11-26 AM: check if git-ai bg run stopped respawning; if so, stop exact PID and delete metrics-db/{-wal,-shm}
- [2026-11-25 2026-11-25 PTY audit: 211/511 masters, not exhausted](2026-11-25-2026-11-25-pty-audit-211-511-masters-not.dd.md): PTY headroom clear \(211/511 ptmx, max ttys258\); found 0 dead panes, 30 unindexed pij-rs panes \(27 osk\_central\), 11 orphan crates/tmux sleep PIDs
- [2026-11-25 2026-11-25 ranked reclaim list](2026-11-25-2026-11-25-ranked-reclaim-list.dd.md): Targeted survey done: .git-ai/metrics-db 24G live grower, umx-work targets 23.7G, pij.sqlite +1.7G post-0052, Docker 68.9G reclaimable
<!-- /pij:generated -->
