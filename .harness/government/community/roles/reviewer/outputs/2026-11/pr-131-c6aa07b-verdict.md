# PR #131 covering-index repair review

## Verdict

**pass-with-notes**, exact head `c6aa07bfeb7316ee277f7325f873c3684e8eb4fa`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/131#issuecomment-6675476661

The four-file `93ca2b1..c6aa07b` delta removes the card's 48-row truncation and preserves complete addresses for the selected three distinct PRs. No new blocking correctness issue found. The original F001–F003/N001 closures carry forward; F004 remains withdrawn. N002 below is a nonblocking metadata-provenance qualification.

Reviewed only this delta and the record/card paths it touches, in the fresh frozen `review-131-c6aa07b` worktree. CI first: no local check was needed because the changed SQL, query plans and large-delivery behavior are covered by exact-head CI. No local cargo/just/test/probe run, product edit, commit, push or deployment.

## Selector and address-read verification

- Migration 0033 now defines `deliveries_by_pm(pm, recorded_at, project, pr)`, retaining its non-null PM predicate. The first selector projects only covered columns and orders timestamp buckets/project/PR in index order.
- `PM_DELIVERY_STEP_SQL` seeks the next older timestamp and returns distinct project/PR pairs within it. The caller decreases the timestamp cursor, deduplicates PRs repeated by later address additions, and stops at three distinct PRs or exhaustion. It requests up to the full result limit per bucket, rather than only the remaining slots, so already-seen PRs do not hide unseen peers at the same timestamp.
- `DELIVERY_ADDRESSES_SQL` then reads every address for each selected `(project, pr)` through the primary key, in address order. The address count no longer consumes the PR-selection limit. The earlier fewer-than-three/partial-address summary note is resolved.
- Selection and address reads share one read transaction, preventing a mixed snapshot while another delivery is recorded. Store errors remain explicit. Empty history and limit zero return no notes; timestamp ties have deterministic project/PR ordering.
- The new query-plan control uses the actual production SQL and bound arguments. CI confirms the covering PM index for selection, the deliveries primary key for addresses, and no TEMP B-TREE in either plan.
- The new 60-address regression checks all three PRs, all sixty newest-PR addresses, order, timestamp and merge commit. The ordinary card/store case and four original reviewer HTTP regressions also pass at this head.

**Cost:** these are two indexed phases, not necessarily two SQL statements: timestamp stepping may perform several seeks, followed by one address read per selected PR. Work scales with relevant selected-PR delivery rows/addresses rather than sorting the PM's full history. The loop is request-scoped; no new periodic/per-seat bulk work or launch is added. `/v1/cards` still skips this lookup, singular `pij state` retains it, and the delivered verb's gh launch count is unchanged.

## N002 (P3): nonblocking recorder/timestamp provenance

At `crates/store/src/claims.rs:426-444`, `recorded_at` comes from the newest selection bucket, while `recorded_by` comes from the first address in primary-key order.

If a PR first records req-20 by seat A at time 10, then a later invocation adds req-21 by seat B at time 20, its card can combine timestamp 20 with recorder A because req-20 sorts first. No address or stored receipt is lost; the two values simply need not describe the same recording operation.

This is not a merge blocker for the performance repair. A follow-up should either select recorder metadata from a row matching the selected timestamp (the address read can include each row's timestamp), or explicitly define the aggregate card's recorder policy. Add a later-address/different-recorder case alongside the large-PR fixture. This qualification is source-derived, not a claimed local reproduction.

## CI evidence

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42151520958

Exact reviewed SHA; all four jobs pass: Node 22, Node 24, rust-lint and rust-test. Rust nextest summary: **1,915 passed, five skipped**.

Published `junit-linux` receipts confirm:

- `the_pm_card_reads_walk_their_indexes_without_sorting_the_history`;
- `a_large_newest_delivery_never_hides_an_address_or_an_older_pr`;
- `deliveries_record_once_release_named_claims_and_reach_the_pm_card`;
- all four adopted reviewer HTTP regressions.

The complete CI jobs also pass workspace fmt/clippy, locked metadata, doctests and the Node typecheck/lint/tests/native contracts. Evidence was obtained from GitHub run metadata, the rust job log and the published JUnit artifact; those are views of one CI origin, not independent runtime measurements. No full local gate or live cost measurement is implied.

Final pin inspection: GitHub and the clean detached worktree remain at `c6aa07b`; PR is CLEAN. The PR lane remained untouched. No local CI gap justified consuming the niced lane build slot.

## Handover and harness gift

No merge-blocking change is owed for this delta. The previous row-window completeness qualification is closed by the new selector and the executable 60-address CI fixture. N002 is a nonblocking provenance follow-up; performance's deployment/live receipts remain separately owned.

**magicWand outcome:** both production query phases now have plan guards, and the formerly missing oversized-delivery fixture runs in CI. Extend that same fixture family to timestamp ties and address-adding retries so aggregate metadata policy is equally explicit.

**difficulties:** none required a local workaround; CI supplied the exact query-plan and large-delivery receipts.
