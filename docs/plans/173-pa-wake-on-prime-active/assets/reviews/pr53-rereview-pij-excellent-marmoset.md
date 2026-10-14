# PR #53 — frozen-scope re-review

- **Verdict: REQUEST_CHANGES.**
- **Reviewer:** `pij-excellent-marmoset`, OMP / `github-copilot/gpt-6-astra`.
- **Subject:** `bd06e492afa611b38c1c77c0dc14d614575e956e`; fix diff from `6e99ede729503b320f74e1c92479b2b1e1ad61d5`.
- **Scope:** F001–F004 and regressions introduced by their fixes only.
- **Read-only checkout:** `/Users/jordanknight/pi-hacking/fleet-review-53`.
- **Isolated execution copy:** `/private/tmp/pij-excellent-marmoset-173-rereview-8ufkbd0n`.
- **Plan:** `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json`; guide: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/impl-guide.dd.json`; tasks: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json`; execution log: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/execution-log.dd.json#entries`.

## A) Verdict

The three adopted reviewer regressions **pass**, as do all **24 watchdog tests**. F002 and F003 are fixed. F001's original wrong-prime wake is fixed, but its correction introduces a missed-wake boundary. F004 remains partially fixed: canonical gate assertions still contradict the local Rust receipt, and the new `Simple` mode selects a nonexistent task section.

**Requested fresh mutation proof is blocked, not passed.** I applied the identity mutation in the isolated copy, but the one-minute load remained above 40 for the entire 1200-second launch gate; no mutant test process launched. I restored the guard and verified source bytes against `bd06e49`. The verdict below rests on fresh baseline/probe execution and the observed contract errors, not on an invented mutation result.

## B) F001–F004 dispositions

| Finding | Fixed? | Verification / regression disposition |
|---|---|---|
| **F001 — prime identity binding** | Original defect fixed; **not cleared** | The adopted replacement-working regression and spent-cap replacement case pass. `armed_at_ms(prime)` and `started` now enforce identity. However, a replacement first observed **idle** loses that valid arming tick; its next self-start sends no wake. R1 below. |
| **F002 — positive idle evidence** | **Fixed** | Both adopted unknown-observation regressions pass. Raw PA/prime `SystemState` reaches core; arming explicitly requires both `Idle`. Non-idle prime observations invalidate an armed episode without resetting `Spent`. No additional F002 regression found in the inspected change. |
| **F003 — documentation accuracy** | **Fixed for the reported corrections** | The doc distinguishes default PA wakes from limit-reset fallback/opt-ins, states the actual tombstoned-parent anchor behavior, and names the lowest-id reset target. The PA brief qualifies peer-caused turns with the receipt boundary and residual. No separate documentation regression found; the new arming description is affected by R1's implementation mismatch. |
| **F004 — proof claims / metadata** | **Partially fixed** | Smoke is explicitly unrun; fake core-unit-test proof is replaced with the real daemon sensor; review/dispositions and `meta.log` are recorded. But “each/every stage passes except smoke” still contradicts `lg-0009`, and `mode: Simple` is incompatible with the external phase task dossier. R2/R3 below. |

The previous independent guide/decomposition review remains recorded at `assets/reviews/pr53-pij-excellent-marmoset.md`; this pass inspected only its F004 changes, not a new design scope.

## C) Remaining in-scope findings

### R1 — HIGH: F001 fix discards an eligible replacement-idle tick

**Location:** `/Users/jordanknight/pi-hacking/fleet-review-53/crates/core/src/watchdog.rs:204-235`, especially `214` and `230-234`.

Observed sequence:

1. Worker idle edge opens the episode; A and the PA are idle at tick 2, arming for A.
2. B becomes designated between ticks and is **idle at tick 3**, with the PA also idle.
3. B starts working at tick 4, with no delivery receipt causing the turn.
4. **Observed: 0 wakes. Expected: 1.**

At tick 3, the tracker is `Armed` and `same == false`. It falls through to `Open` instead of binding/arming from B's fresh positive idle observation. At tick 4, `(Open, Working)` waits. Thus the fix requires an extra idle observation of B and misses the first otherwise-eligible self-start. This is directly adjacent to F001, not an unrelated feature request: the existing contract says arm on the first eligible post-edge idle tick, and the revised doc says the new prime must itself be observed idle and then working.

Fresh failure: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-idle-replacement.txt:16-33`. The test ran after the build resumed; no pause occurred during its 0.94-second execution. Probe: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-idle-replacement-probe.rs`.

**Smallest correction:** on identity change, discard A's cursor but allow the current observation to arm for B when B and the PA are positively idle, using the current tick's timestamp. Never wake B on the identity-change observation itself; preserve `Spent`. Keep the working-replacement and spent-cap tests.

### R2 — MEDIUM: F004's gate assertion is still stronger than its receipt

**Locations:** `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json:100-104`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json:149-153`.

The checked AC still says **“Every harness checks stage passes except smoke”**; the checked done-when says **“Each harness checks stage passes … except smoke”**. But `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/execution-log.dd.json:107-114` (`lg-0009`) records the fix-round workspace run at `67b29a3` as **exit 101, 1526 passed, 1 failed**, explicitly **“NOT as a pass.”** The PR prose now correctly preserves that failure; the canonical checked assertions do not.

**Smallest correction:** make those assertions match the observed scoped evidence, including the known local Rust failure, named smoke exclusion, and separately green exact-head CI. Do not convert the failure into a pass or rerun unrelated tmux tests for this review.

### R3 — MEDIUM: F004's new `Simple` mode selects missing tasks

**Locations:** `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json:15,109-116`.

The builder review contract resolves Simple-mode tasks at `plan.dd.json#tasks`. This plan has no inline `tasks`; its existing `ph-a578` references `assets/tasks/phase-1/tasks.dd.json#tasks`. Actual command:

```sh
harness plan validate /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json --address /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json#tasks
```

**Observed:** exit 1, `E458`, `the document has no section "tasks"`.

The same validation with the existing phase-task address returns **status ok, 0 errors, 0 warnings**. This is a metadata regression in the F004 correction, not a request to restructure the plan.

**Smallest correction:** use `meta.mode: Full` to match the existing phase/dossier layout and retain the declared logs. No new phase or task decomposition is needed.

## D) Independent execution evidence

### Adopted regressions — all pass

- `a_replacement_prime_must_be_observed_idle_before_it_wakes_the_pa`
- `an_unknown_pa_observation_never_arms_the_wake`
- `an_unknown_prime_observation_never_arms_the_wake`

The full selected suite also passed the replacement spent-cap test, all three fail-safe receipt boundaries, original brake/cap/between-ticks/restart cases, two-PA scenario and existing watchdog neighbors: **24 passed, 0 failed**. Receipt: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-baseline.txt:33-63`.

### Fresh mutation attempt — unfulfilled due to C12

Mutation applied only to the isolated copy: identity comparison at `watchdog.rs:194` → `let same = true;`. The launch wrapper waited only on **the 1-minute average**, not `max(1,5,15)`. The condition never cleared during the 1200-second launch deadline:

```text
C12 blocked launch for entire deadline: load=(64.32568359375, 72.17333984375, 68.779296875)
exit 75
```

No red test ran, and no post-mutation green run is claimed. The guard and temporary probe were removed/restored; `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-restoration-and-blocker.json` verifies all three inspected Rust source files byte-match `bd06e49`. Baseline green precedes this mutation attempt. Completing the requested red/restore/green demonstration requires host headroom permitted by C12.

### Exact-head CI

`check (22)`, `check (24)`, `rust-lint`, and `rust-test` pass. The [run API](https://github.com/AI-Substrate/pij-fleet/actions/runs/38493586785) confirms `head_sha: bd06e492afa611b38c1c77c0dc14d614575e956e`, `conclusion: success`. This does not supersede the new R1 failing probe or relabel the local Rust failure.

## E) Scope and proof limits

No production daemon/store access, live panes, smoke, project commits or pushes. No unrelated repository audit. Builds used the authorized warm target, `-j 1`, `nice -n 19`, `timeout -k 10 1200`, and the 1-minute load gate. Baseline compilation was paused when that average exceeded 40; the 5/15-minute averages were recorded but did not gate. The supplied checkout was not edited.

No full `harness checks` claim. This pass proves composed watchdog/temporary-store behavior and task-address resolution; it does not prove a live PA consumes a wake. The local `ddocs` executable was absent, so the task-address checks used the installed `harness plan validate` command instead.

## F) Follow-up only

No additional product findings outside F001–F004. Harness friction: shared package-cache locks and sustained host pressure delayed or prevented isolated verification. The updated scratchpad wrapper encodes the requested one-minute gate; those environment observations are not product passes or reasons to weaken C12.

## G) Commands / artifacts

Execution copy: `/private/tmp/pij-excellent-marmoset-173-rereview-8ufkbd0n`. Common test wrapper: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-c12-run.py`; environment: `CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-173-prime-active/target`.

```sh
cargo test --locked -j 1 -p pij-daemon --lib pa_watchdog -- --test-threads=1
cargo test --locked -j 1 -p pij-daemon --lib pa_watchdog::tests::rereview_idle_replacement -- --test-threads=1
harness plan validate /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json --address /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json#tasks
harness plan validate /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json --address /Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json#tasks
gh pr checks 53 --repo AI-Substrate/pij-fleet
gh api repos/AI-Substrate/pij-fleet/actions/runs/38493586785 --jq '{head_sha,conclusion,status,html_url}'
```

- Scoped diff: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-pij-excellent-marmoset.diff`.
- Baseline: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-baseline.txt`.
- F001 regression probe/result: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-idle-replacement-probe.rs`, `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-idle-replacement.txt`.
- Restoration and external blocker: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/rereview-restoration-and-blocker.json`.

## H) Handover

**REQUEST_CHANGES.** F002/F003 clear; F001 needs the R1 boundary correction; F004 needs R2/R3 factual/metadata corrections. Preserve all 24 adopted tests. Add the R1 probe to the same daemon test module, prove it fails on `bd06e49`, then passes with the correction. Re-review remains scoped to those corrections and regressions they introduce. Fresh independent identity/positive-idle mutation evidence is still explicitly outstanding because the required C12 launch condition was unavailable.
