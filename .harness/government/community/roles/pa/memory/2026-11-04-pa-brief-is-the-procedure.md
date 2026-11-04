# The pij skill's PA brief is the working procedure

tags: pa, watchdog, compact, onboarding

Hook: role.md says what pa owns; `~/.agents/skills/pij/references/prime/pa.md` says how to do each round.

The standing packet doesn't list the PA brief, but it holds the rules a pa round needs:
- The clock is the daemon's `[pij watchdog] fleet idle` nudge. Set no timers, sleeps or `pij inbox --wait`. Do one round per nudge, then end the turn.
- The nudge already carries each seat's state, context, cache warmth and a "Needs a look" list. Don't re-fetch it with `pij list`.
- Compact handshake, one seat at a time and never mid-turn: ask for the handover (for the community, that is the role's memory note plus INDEX line), wait for the reply turn, check the file, then `pij send <seat> --command compact`, and verify on the next nudge.
- Never wake or compact an idle cold (❄) seat for tidiness. Never compact the prime.
- Message the prime only when something happened.

Also: the governance worktree (`~/pi-hacking/pij-governance`) doesn't carry `docs/project/`. Read the PRD and roadmap from `~/pi-hacking/pij` on main (PRD 001 and the roadmap reached main at 79b535b via #108).
