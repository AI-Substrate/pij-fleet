# Plan 175 rulings (prime, 2026-10-16)

These settle the judgement calls in the survey. Anything else ambiguous goes to the prime.

| id | topic | ruling |
|---|---|---|
| R1 | pair route | Rewrite pair.md to current practice: builder implement + builder review in separate seats through the peer route; delete flow-pair machinery. Keep the /flow-pair alias. |
| R2 | governance root | pij-fleet has no governance worktree today. Document the absent case and drop dead examples; do NOT create a governance branch or worktree (that is Jordan's call, asked separately). |
| R3 | chore | `pij-rs sidecar chore-*` stays unadvertised (docs/how/pij-rs-api.md deliberately omits it); the skill keeps `chore` as a named refusal. |
| R4 | idle seats | protocol.md wins: never message an idle seat just to check on it. Remove orchestrator's 'poke once'. A stale card on an idle/done seat is not a nudge trigger; nudge only seats that are working or owe a card. |
| R5 | compaction | No seat compacts a seat it does not govern. The recorded parent's C3 completion compaction and the PA case pa.md already documents are the only cases, and protocol.md says so instead of 'nobody'. |
| R6 | the-flow files | Invariant 1 keeps the-flow.json/the-flow.md as never-hand-written (the `harness flow` CLI is their only writer) and drops `.the-flow-state.json`, which no longer exists. |
| R7 | #55 | PR #56/#57 are not merged: document nothing from plan 174 (no E-RS-CONVERSATION-LIVE). Keep the manual check-before-revive step. |
