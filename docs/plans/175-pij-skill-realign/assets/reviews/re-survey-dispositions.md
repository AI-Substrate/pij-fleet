# Re-survey dispositions (fix round 1)

Review: [re-survey-gpt-6-sol.md](re-survey-gpt-6-sol.md) (Copilot gpt-6-sol, subject 617fc02, REQUEST CHANGES). Prime's fix packet adds ruling R8 (`../rulings.md`). Lines are under `skills/pij/` after the fix.

| # | review row | verdict | disposition | where |
|---|---|---|---|---|
| rv-01 | pair.md:78-85 OMP spawn into a worktree | WRONG | FIXED. Re-verified at `crates/cli/src/main.rs:2650`: `pij-rs spawn` refuses `omp`/`pi` with a linked-worktree `--cwd` before the daemon call. The rule is stated once in the peer route; every worktree spawn example uses `claude`/`copilot` and cites it | references/routes/peer.md:46 (rule); references/routes/pair.md:79, :85 |
| rv-02 | pair.md:33-35, 114-127 approval report before `harness checks` | WRONG | FIXED. The gate runs before approval: the APPROVE row, the report comment and Procedure 5-6 are reordered; orchestrator's stop report now says it comes only after `harness checks` passes | references/routes/pair.md:35, :61, :126; references/prime/orchestrator.md:17 |
| rv-03 | delegate.md:36 worktree spawn | WRONG | FIXED (cites the rule) | references/routes/delegate.md:37 |
| rv-04 | orchestrator.md:55-64 default OMP coder in a worktree | WRONG | FIXED. The default coder is Copilot gpt-5.6-sol at xhigh; OMP/Pi are excluded for worktrees, and the Copilot compaction caveat is stated | references/prime/orchestrator.md:56, :64 |
| rv-05 | kickoff.md:15-19 OMP/Pi stream spawn | WRONG | FIXED. Uses `claude\|copilot`; OMP/Pi spawn from main with the worktree path in the brief | references/prime/rituals/kickoff.md:18 |
| rv-06 | kickoff.md:46-51 mandatory canary file under an absent root | WRONG | RULED R8. The `pij canary` store receipt is the record | references/prime/rituals/kickoff.md:52; references/prime/rituals/bootstrap.md:48 |
| rv-07 | incidents.md:20-24 incident file under an absent root | WRONG | RULED R8. The plan's `assets/incidents/`, or a stripped pij-fleet issue | references/prime/rituals/incidents.md:25 |
| rv-08 | stream-brief.md:29-32 dead fence pointer | MISSING | RULED R8. The fence pointer is `pij fence show --stream <id>` | references/prime/templates/stream-brief.md:31 |
| rv-09 | orient-global.md:37-39 canary record destination | MISSING | RULED R8. File, or the store receipt | references/prime/orient-global.md:40 |
| rv-10 | node.md:53-65 `report now --note` | MISSING | FIXED. Re-verified at `crates/daemon/src/http/report.rs:224-240`: only with `--state question\|blocked` | references/routes/node.md:55, :65 |
| rv-11 | 00-routing:216 plan 083 figure | UNVERIFIABLE | UNVERIFIABLE (kept), now attributed: "pre-OSS plan 083, not in this checkout (operator-reported…)". Ledger row co-38 relabelled from REJECTED | references/00-routing.md:216 |
| rv-12 | 00-routing:248 load ~230 anecdote | UNVERIFIABLE | UNVERIFIABLE (kept), attributed "By operator report" (co-39) | references/00-routing.md:248 |
| rv-13 | protocol.md:115 RCA figures | UNVERIFIABLE | UNVERIFIABLE (kept), already attributed to the named RCA and its workspace (pc-28) | references/prime/protocol.md:116 |

Gate notes (review § Gates, judgements a-c): no change needed. The review recommends wiring the link checker later; that stays the retro's magicWand.
