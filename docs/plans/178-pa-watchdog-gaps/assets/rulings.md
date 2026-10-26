# Plan 178 rulings (prime, 2026-10-26)

| id | topic | ruling |
|---|---|---|
| R1 | scope suppression | Keep 'any stopped Claude seat pauses its scope's idle and prime-active handling' while the limit is current; only stale (past-reset) rejections stop counting. The brief is reworded to match the code, not the reverse. |
| R2 | PA self-compaction | The PA's cap is 600k by default, the same as for seats, overridable by the prime in the stand-up packet; it compacts itself with compact-self at the end of a round, while warm. |
| R3 | questions | SKILL invariant 10 wins: the PA asks Jordan inline for decisions it owns; a question about another seat's work belongs to that seat or the prime. |
| R4 | git | No git writes by the PA, except removing a worktree the prime named in an order. Line ~9 says so. |
| R5 | Copilot and paneless | The PA does not try --command compact on them; it reports the size to the prime. |
