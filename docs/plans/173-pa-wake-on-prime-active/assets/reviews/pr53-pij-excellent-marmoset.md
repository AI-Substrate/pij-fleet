# PR #53 — independent code and guide review

- **Reviewer:** `pij-excellent-marmoset`; requested OMP / `github-copilot/gpt-6-astra`; runtime advertises that model with high reasoning. Provider identity is not independently attested.
- **Date:** 2026-10-14 UTC.
- **Subject:** `19953b224f2aff8ea17e045a33c8b2b7427acac9..6e99ede729503b320f74e1c92479b2b1e1ad61d5`; [PR #53](https://github.com/AI-Substrate/pij-fleet/pull/53).
- **Plan:** `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json`.
- **Guide:** `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/impl-guide.dd.json`.
- **Phase:** `ph-a578` — PA wakes once on prime activity after a worker-idle episode.
- **Plan SHA-256:** `5682d0c4f3f9591cfd45efa59647f3ab5072aaac5ca3b12c79d2ebad2c4cf294`.
- **Guide SHA-256:** `99dab98cf15cb5ab3d5791006dd387cb34971bb30bcf851fba776fea6dd953e7`.
- **Boundary:** supplied worktree read-only. All mutations/probes ran in `/private/tmp/pij-excellent-marmoset-173-6mg2qxw8`. No production daemon/store access, live panes, smoke, project commits or pushes. The prime subsequently authorized the coder's warm target directory and `-j 1`.

## A) Verdict

**REQUEST_CHANGES — 2 HIGH, 2 MEDIUM.**

The brake, cap, fail-safe receipt reads, between-ticks report case and restart behavior work in the exercised cases. Arming violates AC1/AC2 in two independently reproduced ways: observations are transferred across prime identities, and unavailable states count as idle. Three new regressions fail against the unmodified implementation. The existing 17 watchdog tests pass after restoration; that suite does not cover these defects.

## B) Summary and independent guide/decomposition review

**I independently reviewed the guide decomposition in this review.** This is a retrospective independent review, not a claim that approval occurred before implementation.

One cohesive unit, one composition root and five ordered tasks are appropriate: the tracker/daemon share an episode lifecycle, and splitting them among coders would add coordination without an independent interface consumer. Core owns decisions; the daemon owns registry/spine reads and delivery. The change reuses `IdleEdge`, `fleet_working`, indexed `SpineWindow` queries, the existing delivery receipt kind and cold-wake audit path. No second scheduler or receipt authority was introduced.

The decomposition omitted prime-identity and non-idle-observation hazards. Its proof map overstates a new core-unit-test sensor: the added behavioral tests are in the daemon, not core. `vd-0003` proves only that the doc is nonempty; its content needs this independent review. `vd-0004`/AC9 cannot represent a full gate pass while the receipt explicitly excludes smoke. See F004.

Input-contract gaps: the product plan has no `meta.mode`; neither plan nor task dossier declares `meta.log`. I recovered the exact phase and execution entries from the explicit phase/task/AC links. The current builder review contract expects those metadata fields; schema acceptance alone does not supply them. The guide's `baseline.receipt` is a team-style path, not observed baseline proof; this review instead binds the explicit base/head above.

## C) Checklist

- [x] Exact requested base/head diff saved; plan, guide, tasks, execution log, backpressure, PR body and coder retrospective read.
- [x] Independent guide/decomposition review recorded.
- [x] Existing composed behavior exercised using temporary SQLite stores and registry fixtures.
- [x] Reviewer-authored negative cases executed against unchanged implementation.
- [x] Brake physically disabled: tests red; guard restored: all 17 existing tests green.
- [x] Exact-head CI inspected: four checks pass on `6e99ede729503b320f74e1c92479b2b1e1ad61d5`.
- [ ] AC1/AC2 fully satisfied — F001/F002.
- [ ] Shipped-behavior docs entirely accurate — F003.
- [ ] Full `harness checks` proven — F004; smoke prohibited, not run or counted as passing.
- Domain mode: **N/A** — no domain manifest in this plan; existing domain dossiers do not establish a domain-mode contract for the Rust watchdog.

## D) Findings table

| ID | Severity | Location | Finding | Required disposition |
|---|---|---|---|---|
| F001 | HIGH | `/Users/jordanknight/pi-hacking/fleet-review-53/crates/daemon/src/pa_watchdog.rs:183-188,408-440`; `/Users/jordanknight/pi-hacking/fleet-review-53/crates/core/src/watchdog.rs:170-173,184-219` | An idle observation of prime A arms a wake for already-working replacement prime B. | Bind arming to prime identity; invalidate the old idle observation/cursor on identity change without resetting the episode cap. |
| F002 | HIGH | `/Users/jordanknight/pi-hacking/fleet-review-53/crates/daemon/src/pa_watchdog.rs:409-411,435` | `Unknown` and other non-working states become observed idle, contrary to AC2. | Require PA `Idle`; distinguish prime `Idle`/`Working` from unavailable/non-idle observations. |
| F003 | MEDIUM | `/Users/jordanknight/pi-hacking/fleet-review-53/docs/how/pa-watchdog.md:3,15,54,63,85-87`; `/Users/jordanknight/pi-hacking/fleet-review-53/skills/pij/references/prime/pa.md:18-21` | New documentation contains exclusive-target and causal guarantees that the code does not make. | Correct default-vs-opt-in/reset targets, actual parent anchoring, and the residual qualification. |
| F004 | MEDIUM | `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/plan.dd.json:97-102`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json:143-150`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/impl-guide.dd.json:202-215`; `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/backpressure.dd.json:18-27` | Checked proof claims overstate the full gate and core tracker tests. | Record scoped gate evidence/exception honestly; map proof to actual tests and reconcile builder metadata. |

## E) Detailed judgments

### F001 — prime identity is missing from arming state

Reproduction: B is a `prime`-role supervisor and is working throughout. After a worker idle edge, A and the PA are observed idle, arming the tracker. Retire A's designation and designate B between ticks. The next tick sends a wake although B never transitioned idle→working and was never the prime observed idle at arming.

`scope_prime` correctly selects B, but `PaTrackers`/`PrimeActive` retain only episode/time and a boolean, not the identity associated with them. B inherits A's arming evidence and spends the one wake. This can waste the episode's morning wake without any human return. The fix must retain `Spent` across identity changes: resetting the whole tracker/cap would create a new supervisor-loop risk.

Fresh failing test: `review_replacement_prime_must_be_observed_idle_before_waking` in `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-adversarial-tests.rs`. The corrected fixture retires A before designation and asserts B actually became current; an earlier fixture merely received `GovernanceOutcome::Conflict` and was not valid proof. Authoritative failure: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-adversarial-corrected.txt:18-21`.

### F002 — non-working is not observed idle

`prime.state == Working` maps `Unknown` to `Some(false)`; `pa.state != Working` maps `Unknown` to `pa_idle = true`. Thus either unavailable observation can arm the trigger. The next prime working observation sends and spends the cap even though no post-edge tick observed both participants idle.

Two fresh failures: `review_unknown_pa_observation_must_not_arm` and `review_unknown_prime_observation_must_not_arm`; `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-adversarial-corrected.txt:24-32`. `SystemState::Unknown` explicitly means mechanical observation unavailable (`/Users/jordanknight/pi-hacking/fleet-review-53/crates/core/src/model.rs:157-175`). The same comparisons also collapse the other non-working variants. Require positive idle evidence; unknown must not authorize a wake.

### F003 — documentation accuracy

Most of the new guide accurately describes the tracker, cap, indexed receipt window, clock/reset margin and audits. Three statements need correction:

1. “It wakes no worker and no prime” contradicts its own limit-reset fallback and opt-in sections. The passing existing tests `with_no_pa_a_late_reset_wakes_the_designated_prime_at_once` and `an_opted_in_seat_is_nudged_when_quiet_and_told_how_to_stop` exercise those exceptions. The correct exclusive statement is about default idle/prime-active targets, not the whole daemon round.
2. “With no live parent” is not the implemented scope fallback. `scope_of` chooses any recorded parent found in the unfiltered registry list, including tombstones (`/Users/jordanknight/pi-hacking/fleet-review-53/crates/daemon/src/pa_watchdog.rs:819-835`; `/Users/jordanknight/pi-hacking/fleet-review-53/crates/store/src/registry.rs:421-440`). This is an existing implementation detail newly misstated by this document, not a request to expand this PR's behavior.
3. The PA brief says a peer-caused prime turn “never” sends this wake. The accepted pre-arming receipt residual explicitly disproves that absolute. Qualify it with the receipt-since-arming boundary and link the residual.

### F004 — proof must preserve its limits

`ac-0009` and task `dw-0007` say `harness checks` is green/passes. Execution entry `lg-0004` explicitly says **smoke NOT RUN** under a named no-live-pane exception. That exception is legitimate; representing the full gate as passed is not. Retain the exception and amend the claim/state to the actually observed scope.

The guide's core tracker test check and backpressure text claim additions under core, but no `PrimeActive` or `scope_prime` tests were added there. Actual behavior proof is the daemon's 17 scenarios. Correct the sensor mapping rather than adding redundant tests to satisfy a label. The three new boundary regressions are justified tests because they expose user-visible wrong wakes.

### Requested eight judgments

1. **Can the brake increase wake frequency?** Not in the scheduled-wake count per fixed worker-idle episode. At a would-be trigger it changes `Wake` to `Braked`, leaving the cap available; removing it spends the cap earlier. It can defer the sole wake, not create a second one. This is the requested one-directional **brake**, not a policy. This count claim is not a claim that wake timestamps remain identical or form a pointwise subset.
2. **Failed/exhausted receipt reads fail safe?** Yes for window-construction errors, store query errors and the four-page ceiling. All three were independently exercised: invalid `u64::MAX` boundary; a deliberately removed table in a fresh temporary database; 1024 queued receipts. With 1023 queued rows the reader reaches a proven end and returns false; at 1024 it returns true. This does not claim malformed JSON payloads are fail-closed: the parser currently ignores decode failures, while the normal publisher serializes the typed receipt.
3. **AC3 disarm-keeps-cap?** Yes. `Braked` transitions to `Open`, not `Spent`. The existing scenario produces `braked`, then `sent` after a fresh idle pair and self-start. Independently green after restoration.
4. **Report between ticks?** The named post-arming delivery case works and the brake mutation makes it red. General arming is not correct because of F001/F002. The pre-arming receipt residual is separate and explicitly documented.
5. **#52 F1 / no supervisor loop?** Preserved for the existing stable scope/role episode: `fleet_working` excludes every PA, every prime-role seat and the current designated prime. Supervisor turns do not produce the worker edge that resets the cap. The updated two-PA/two-episode test and repeated-prime-turn cap test pass. F001/F002 waste a bounded wake; they do not defeat the per-episode cap.
6. **Restart only arms?** Yes: `IdleEdge` initially records state without emitting an edge; `PrimeActive` remains `None` until a later real worker edge. Repeated prime activity after restart sends nothing. A new worker episode enables the feature again; the restart scenario passes.
7. **Four coder decisions:** no-prime on the existing edge's `prime_active` field is an acceptable audit shape and avoids a duplicate event; the literal reason `prime-active` matches the requested contract and existing reset convention; the stated pre-arming residual is within the explicit receipt boundary and bounded by the cap, but may consume the one morning wake; R7 is correctly deferred by the explicit non-goal. None justifies F001/F002.
8. **Whole watchdog doc accurate?** Mostly; F003 must be corrected. The state diagram, brake/cap behavior, limit reset timing and cold-wake path match the examined code.

## F) Coverage map

Confidence below is confidence in the disposition, not a percentage of scenarios exhaustively explored.

| AC | Disposition | Fresh evidence | Confidence |
|---|---|---|---|
| AC1 | Ordinary overnight resume passes; replacement-prime counterexample remains | Existing overnight test green; F001 regression red | 99% |
| AC2 | Not satisfied generally | Before-arming/no-prime tests green; three arming regressions red | 99% |
| AC3 | Satisfied for stated receipt-after-arming boundary | Brake/cap-preservation and between-ticks tests green; independent brake deletion red | 99% |
| AC4 | Satisfied in exercised episodes | Repeated-prime and two-PA tests green; cap mutation receipt read, not independently repeated | 98% |
| AC5 | Satisfied at queue/audit boundary | Overnight test checks digest, cold reason/audit; no-prime edge audit test green | 97% |
| AC6 | Satisfied | Usage-limit test green; `idle-on-limit` then later `sent` | 99% |
| AC7 | Satisfied | Restart scenario green, no pre-episode wake | 99% |
| AC8 | Needs correction | Source comparison and passing opt-in/reset exceptions; F003 | 99% |
| AC9 | Scoped evidence strong; full-gate claim overstated | Fresh mutation/restoration; exact-head CI green; full local gate not proven | 99% |

**Proof ceiling:** composed `PaWatchdog::round`, real temporary SQLite/queue, injected registry/state/time/status inputs. Queue admission is not a running PA consuming a message. No live-runtime or production smoke evidence is claimed.

## G) Commands and evidence

Exact diff was produced with `git diff 19953b224f2aff8ea17e045a33c8b2b7427acac9 6e99ede729503b320f74e1c92479b2b1e1ad61d5` and saved to `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-pij-excellent-marmoset.diff`. The subject was exported with `git archive 6e99ede729503b320f74e1c92479b2b1e1ad61d5` into the isolated directory.

The successful warm-target test invocations were wrapped by `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-c12-run.py`, which invokes `nice -n 19 timeout -k 10 1200`, limits Cargo to one job, and pauses the owned process group when observed load exceeds 40. The original `-j 4` cold-build attempts stopped at the cap; the prime then authorized warm-target reuse and `-j 1`.

```sh
# cwd: /private/tmp/pij-excellent-marmoset-173-6mg2qxw8
# environment: CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-173-prime-active/target
cargo test --locked -j 1 -p pij-daemon --lib pa_watchdog::tests::review_ -- --test-threads=4
cargo test --locked -j 1 -p pij-daemon --lib pa_watchdog -- --test-threads=4
cargo test --locked -j 1 -p pij-daemon --lib pa_watchdog -- --test-threads=1

gh pr checks 53 --repo AI-Substrate/pij-fleet
gh api repos/AI-Substrate/pij-fleet/actions/runs/38487290208 --jq '{head_sha,conclusion,status,html_url}'
```

| Receipt | Observed result |
|---|---|
| `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-adversarial-corrected.txt` | Unchanged implementation plus six reviewer probes: 3 fail-safe probes pass; replacement identity and two unknown-observation regressions fail; exit 101. |
| `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-mutation-brake-red-final.txt` | Only mutation: `else if seen.delivered_since_arming` → `else if false`; 15 passed, 2 brake tests failed; exit 101. |
| `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-restored-green-final.txt` | Mutation and added probes removed; exact original implementation/tests restored; 17 passed, 0 failed; exit 0. No pause in this run; maximum observed load 39.32. |
| `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-restoration-integrity.json` | All four Rust files byte-match the requested subject in both supplied worktree and restored isolated copy. |
| [Exact-head CI run 38487290208](https://github.com/AI-Substrate/pij-fleet/actions/runs/38487290208) | `check (22)`, `check (24)`, `rust-lint`, `rust-test` pass; API confirms exact requested SHA. |

Interrupted runs are retained, not counted as passes: `review-mutation-red.txt` stopped at load 40.17; `review-mutation-red-resumed.txt` at 40.70; `review-mutation-brake-red.txt` at 40.79. The first restoration run, `review-restored-green.txt`, paused during the tests at load 42.19 (peak 53.50), resumed, and finished 16/17 with a missing idle nudge in the no-prime fixture. The unpaused serial restoration above passed all 17. [INFERENCE] suspension crossing real timeout boundaries explains that interrupted fixture failure; it is not evidence that the original run passed.

No full `harness checks`, live smoke or unrelated tmux-flake reruns were performed by this reviewer. Historical gate outputs were inspected, not relabeled as fresh reviewer results.

## H) Handover brief

**Result:** REQUEST_CHANGES. Fix F001/F002, correct F003/F004, then re-review the exact new head.

- Product/guide: paths at the top of this report.
- Tasks: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/tasks/phase-1/tasks.dd.json#tasks`.
- Execution evidence: `/Users/jordanknight/pi-hacking/fleet-review-53/docs/plans/173-pa-wake-on-prime-active/assets/execution-log.dd.json#entries` (recovered through explicit task/AC links; no declared `meta.log`).
- Ordered fixes: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/fix-tasks-pij-excellent-marmoset.md`.
- Executable reviewer probes: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173/review-adversarial-tests.rs` — append inside the existing `pa_watchdog_tests.rs` module in an authorized development copy.
- Review/diff/receipts: `/private/tmp/claude-501/-Users-jordanknight-pi-hacking-pij/cbe08128-4097-4473-9f99-71e9415824f6/scratchpad/173`.
- Supplied review worktree remains untouched; isolated source is restored. Shared warm target was used only with the prime's authorization. No mutants remain in source.

**Retrospective / magicWand:** an isolated-review command that accepts an explicitly leased warm Cargo target, enforces the load ceiling, and distinguishes an interrupted test run from a clean verdict. The review supplied the executable C12 wrapper and failing boundary fixtures in this scratchpad. Cold builds hit the cap twice; a later host spike contaminated a paused test run. Preserve the receipt distinction and adopt the boundary fixtures into permanent tests; do not treat either difficulty as a product pass. This is the concrete harness-improvement handoff; no canonical repo ledgers were edited under the read-only contract.
