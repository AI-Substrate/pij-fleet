# PR #53 review: coder dispositions (pij-evident-dog)

Review: [pr53-pij-excellent-marmoset.md](pr53-pij-excellent-marmoset.md) (REQUEST_CHANGES, subject 19953b2..6e99ede). Fix tasks: [pr53-pij-excellent-marmoset-fix-tasks.md](pr53-pij-excellent-marmoset-fix-tasks.md). The prime (pij-far-jackal) accepted all four findings and ruled on them. This was one fix round, with scope frozen.

| ID | Severity | Disposition | Change |
|---|---|---|---|
| F001 | HIGH | Fixed | `PrimeActive` stores the last observed prime `(SeatId, SystemState)`. `armed_at_ms(prime)` answers only for the prime it armed for. If the identity changes, an `Armed` episode drops back to `Open`, and the new prime must itself be seen `idle` with the PA, then `working`. A `Spent` episode stays `Spent`, so the cap survives a new prime. Tests: `a_replacement_prime_must_be_observed_idle_before_it_wakes_the_pa` (the reviewer probe, extended so the replacement's own idle→working fires the one wake) and `a_replacement_prime_keeps_the_spent_cap`. |
| F002 | HIGH | Fixed | `PrimeObservation` now carries the raw `SystemState` of the PA and of the prime. Arming needs both to be `Idle`: `Unknown`, `Starting`, `Stalled`, `Stopped` and `Dead` never arm. While armed, any observation of the prime other than `Idle` or `Working` drops the arming. Tests: `an_unknown_pa_observation_never_arms_the_wake` and `an_unknown_prime_observation_never_arms_the_wake` (the reviewer probes). |
| F003 | MEDIUM | Fixed | `docs/how/pa-watchdog.md` now covers: PA-only fleet wakes, plus the two paths that can reach other seats (the limit-reset wake to a prime, and opt-in stall nudges); the scope anchor as the recorded parent even when tombstoned; the lowest-id PA as the reset target; the residual qualification; and the F001/F002 arming rules. The PA brief qualifies its "never" with the receipt-after-arming boundary. In `docs/how/pij-rs-api.md`, the prime-active paragraph was updated, and its pre-existing "no live parent" line was corrected the same way. |
| F004 | MEDIUM | Fixed | AC9 and dw-0007 now say smoke is NOT RUN under the named no-live-pane exception, and claim no full-gate pass. Guide `vd-0001` maps to the daemon tests that actually exercise the tracker (there are no core unit tests). `vd-0004` and `bp-0003` carry the same exception, and `bp-0001` names the real sensor. Plan `meta.mode` is `Simple`, and `meta.log` is declared on the plan and the task dossier. |

The reviewer's three fail-safe probes are also kept as permanent tests. They pin the brake's fail-safe boundaries: a window that can't be built, a spine read that fails, and running out of pages at 1024 rows.

## Re-review at bd06e49 (last round, verified by the prime)

Re-review: [pr53-rereview-pij-excellent-marmoset.md](pr53-rereview-pij-excellent-marmoset.md). F002 and F003 were cleared. F001 was fixed but its correction introduced R1; F004 was partially fixed, leaving R2 and R3.

| ID | Severity | Disposition | Change |
|---|---|---|---|
| R1 | HIGH | Fixed in 5fa9081 | On an identity change, `PrimeActive::observe` drops the old prime's arming and judges the tick afresh. A new prime observed `Idle` with an `Idle` PA arms on that same tick, and `Spent` still survives. The probe was adopted as `a_replacement_prime_seen_idle_arms_on_that_tick`. It was red on the bd06e49 code (24 passed, 1 failed: 0 wakes, expected 1) and is green after the fix (25/25). Evidence: `evidence/rereview-r1.txt`. |
| R2 | MEDIUM | Fixed | AC9 and dw-0007 now say the local gate is scoped. Smoke was NOT RUN, and the local workspace run at 67b29a3 had 1 pij-tmux failure (lg-0009). That failure is kept as a failure; it is not called a pass. |
| R3 | MEDIUM | Fixed | Plan `meta.mode` is `Full`, matching the phase/task-dossier layout. `harness plan validate` returns ok with 0 errors. |
