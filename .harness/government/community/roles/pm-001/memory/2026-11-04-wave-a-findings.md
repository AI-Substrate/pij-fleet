# Wave A planning findings

tags: numbers, register, governance-writer, ddocs
hook: The PRD register lives in private AI-Substrate/pij, not pij-fleet; moving a checked-out branch's tip corrupts the checkout's next commit.

- `harness builder new` mints `builder/<ord>` branches and reads ordinals only from local worktrees, so it misses plans on branches elsewhere. For `plan/<ord>-<slug>` use `git switch -c` plus `harness plan new <slug> --ordinal <n>`.
- Claims so far: plans 190 to 192, migration 0031, PRD 002 (outputs/LOG.md).
- The register AGENTS.md names (`<government-root>/prd/base-prd.dd.json`) is absent from pij-fleet. It is in private AI-Substrate/pij, branch prime-governance (54 rows, schema prd/requirements).
- ddocs finds schemas at `<gitroot>/.dd`, `.harness/.dd`, `~/.dd`. The orphan governance branch has none. ddocs is not a repo dependency; the global one links to ~/substrate/dd.
- If pij moves prime-governance's tip while a worktree has it checked out, that worktree's index lags and its next commit deletes pij's files. Plan 190 open question 2.
