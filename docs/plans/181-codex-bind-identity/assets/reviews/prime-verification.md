# Plan 181: prime verification

Prime `pij-far-jackal`, 2026-10-28.

- **Diagnosis:** read in full. R9 deviation accepted: Codex writes the rollout only at the first turn, so the binder uses the thread-writer lock, which is the pane's own process holding a file open. Same evidence class.
- **Merge:** PR #81 merged at head `1699bb6` (CI run 40594274912 green) as main `7487114`. No store migration. `refs/notes/*` on origin empty. #76 closed.
- **Deploy:** `just bounce-rs`, daemon pid 30971 → 41991, ping healthy.
- **Live canary, after merge:**
  - `pij-rs spawn --harness codex --model gpt-6.1-sol --effort low --role worker --name c181-prime-canary` returned `ok:true`, seat `pij-shiny-chameleon`, `bound:true`, session `01a17f7a-2779-72a3-a100-450d65c82a10`, pane `%127`, with no timeout.
  - In the seat, `pij whoami` printed `pij-shiny-chameleon`.
  - Its `pij send pij-far-jackal --body-file …` arrived as `from: pij-shiny-chameleon`, body `canary reply: whoami=pij session: pij-shiny-chameleon pane=%127` (spine seq ~2117632).
  - The seat was torn down with owner-close (seq 2118466) and its window killed.
  - Its first send attempt failed only because the prime's instruction used a `--body` flag that the shim doesn't accept. That was a prime error, not a defect.
- **R12 cost, after:** two `pij-rs --json ping` readings 20 s apart, with no pending Codex rows: `lsof.codex-process-files` stayed at 2 (both from the canary's bind) and added 0 launches while idle. Daemon total 2842 → 3100 = 12.9/s, all `ps.proc-liveness` (the death sweep, #74). Matches the coder's "before" (13.65/s).
- **Verdict:** ac-0006 met.
