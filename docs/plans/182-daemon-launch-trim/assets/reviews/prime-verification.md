# Plan 182: prime verification

Prime `pij-far-jackal`, 2026-10-28.

- **Review:** independent seat `pij-review-pr84` at ce27a6a: merge after fixes, 0 blockers, 2 should-fix, 4 nits.
  - It proved DST parity live against `/bin/ps` across a real transition; a current-offset mutant is 1 h off and turns the tests red.
  - All of S1, S2 and N1–N3 were fixed at 65c6f15. N4 → #85.
- **Prime check of S2:** read the diff. `kill(pid, 0)` → ESRCH decides Dead. That is the kernel's own "no such process", the same fact `ps -p` reports. Ok and EPERM still go to `ps`, and pid 0 / >i32::MAX are never signalled. Sound.
- **`nix` in pij-core** is accepted (the resolver needs access(X_OK)).
- **Merge:** PR #84 merged at head `65c6f15` (CI run 40667228005 green) as main `e84da89`. No migration. `refs/notes/*` on origin empty. #74 closed.
- **Deploy:** `just bounce-rs`, daemon pid 30264 → 29541, ping healthy. 75 seats in the roster.
- **Cost (ac-0004), from two `pij-rs --json ping` readings each:**
  - Before, on 17dea30: 16 launches/s in total, with `ps.proc-liveness` at 316 per 20 s = 15.8/s.
  - After, over a steady 60 s on e84da89: 63 launches = 1.05/s. `ps.proc-liveness` is **0** (the op is absent) and `ps.proc-liveness-fallback` is 0.
  - The remainder: 48 are the #86 stuck-delivery retries (tmux reserve + restore at 0.4/s each), 12 are `ps.registration-native-process`, 2 are `date.utc-offset` and 1 is `git.pa-watchdog-common-dir`.
  - The first 30 s after boot showed a burst of 44 `git.pa-watchdog-common-dir`, which did not recur in steady state.
- **`just spawn-rate --seconds 20`:** pij-rs daemon children ≥0.7/s. The machine total was 497/s, none of it pij.
- **Verdict:** ac-0004 met. Close bp-0002 and the phase.
