# PR #53 — review fix tasks

Subject: `19953b224f2aff8ea17e045a33c8b2b7427acac9..6e99ede729503b320f74e1c92479b2b1e1ad61d5`.
Source checkout below is read-only; apply fixes in the coder's authorized worktree, not `/Users/jordanknight/pi-hacking/fleet-review-53`.

## FT-001 — Bind arming to the observed prime (HIGH)

- Files: `/Users/jordanknight/pi-hacking/fleet-review-53/crates/daemon/src/pa_watchdog.rs:183-188,408-440`; `/Users/jordanknight/pi-hacking/fleet-review-53/crates/core/src/watchdog.rs:170-173,184-219`.
- An idle observation of prime A must not authorize a wake when prime B becomes designated while B was already working.
- Carry the watched prime identity with the arming state. On identity change, invalidate the previous idle observation and armed cursor; require a fresh tick where that prime and the PA are idle. Preserve the worker episode and its spent cap: replacing the prime must not reset `Spent` or manufacture a new idle episode.
- Adopt `review_replacement_prime_must_be_observed_idle_before_waking` from the sibling `review-adversarial-tests.rs`. The fixture explicitly retires A, designates B, and verifies B became current before asserting no wake.
- Extend it to observe B idle and then working, proving the still-available wake fires once; also retain a spent-cap replacement case.

## FT-002 — Require observed idle, not merely non-working (HIGH)

- File: `/Users/jordanknight/pi-hacking/fleet-review-53/crates/daemon/src/pa_watchdog.rs:409-411,435`.
- `pa_idle` must mean `pa.state == SystemState::Idle`.
- The prime observation must distinguish `Idle` and `Working` from unavailable/non-idle states. Do not convert `Unknown`, `Starting`, `Stalled`, `Stopped`, or `Dead` into an observed idle tick. Update the observation contract if `None` also represents unavailable evidence.
- Adopt the two `review_unknown_*_observation_must_not_arm` tests from the sibling probe file. Both currently fail on the unmodified PR implementation.
- Preserve the existing before-arming, delivered-report, cap and restart behavior.

## FT-003 — Correct the shipped-behavior documentation (MEDIUM)

- Files: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/how/pa-watchdog.md:3,15,54,63,85-87`; `/Users/jordanknight/pi-hacking/fleet-review-53/skills/pij/references/prime/pa.md:18-21`.
- Replace the absolute claim that the daemon wakes no prime/worker with the actual distinction: PA-only default edge/prime-active wakes; limit-reset may target a prime; opt-ins may target other seats.
- The scope currently follows the recorded parent even if that parent is tombstoned (`scope_of` does not filter the parent). Do not promise fallback on a non-live parent unless behavior is intentionally changed and tested.
- Qualify the PA brief's absolute promise that a peer-caused prime turn never wakes the PA: receipt-after-arming traffic is braked, but the explicitly documented pre-arming residual exists.

## FT-004 — Reconcile proof claims with the receipts (MEDIUM)

- Files: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json:97-102`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json:143-150`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/impl-guide.dd.json:202-215`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/backpressure.dd.json:18-27`.
- Do not mark the full `harness checks` claim proven by a receipt that explicitly says smoke was not run. Preserve the named no-live-pane exception and record scoped gate evidence as scoped, rather than changing an omission into a pass.
- The guide calls `cargo test -p pij-core watchdog` the new tracker's unit-test proof, but no `PrimeActive`/`scope_prime` tests were added under core. Point proof at the actual daemon behavior tests, or add justified missing boundary tests; do not manufacture a unit-test receipt.
- Record this independent guide/decomposition review and the dispositions. Supply the product `meta.mode` and declared execution-log address expected by the current builder review contract; the review recovered evidence from explicit task/AC links, not from a declared `meta.log`.

## Re-review acceptance

- The three reviewer regressions fail on the reviewed head and pass on the fix.
- Preserve fresh brake-deletion red / restoration green evidence and all 17 existing watchdog tests.
- Receipt read/window errors and the 1024-row exhaustion boundary continue to spare wakes.
- Fresh exact-head CI; accurately scoped local gate claims; documentation matches the implementation.
