# PR #131 final card-path review

## Verdict

**pass-with-notes**, exact head `93ca2b1c3dc80d3ea609438e9864d9593b7e35f5`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/131#issuecomment-6674802703

The five-file `09653d3..93ca2b1` delta satisfies the PM's explicit performance handoff. No new blocking correctness issue found. The prior `09653d3` closure of F001–F003 and N001 carries forward; F004 remains withdrawn.

Reviewed in the fresh detached `review-131-93ca2b1` worktree. Source and exact-head CI only under the backoff. No local builds, tests, process probes, mutation runs, boot/checks, daemon scripts or deployment.

## Delta and behavior

- `crates/daemon/src/http/mod.rs:2711-2770`: singular `POST /v1/state` loads deliveries after the common card projection and surfaces store errors explicitly.
- `seat_cards` at :2782 and shared `seat_card` at :2873 no longer call `deliveries_for_pm`. The frequent bulk `/v1/cards` path therefore pays no new delivery-history lookup per seat. Its empty `deliveries` field is intentional and documented, not an accidental loss of the singular state feature.
- `crates/store/src/claims.rs:26-37,392-430`: the singular lookup uses the PM/time index order with `LIMIT 48`, then groups into at most three PR notes in Rust. SQL no longer groups or sorts the PM's full history. The small address sort is over the bounded fetched rows, not the database history.
- The production SQL constant is the same one the new CI EXPLAIN test inspects. That test confirms `deliveries_by_pm` is selected and no temporary b-tree is built.
- Existing card rendering and the singular delivered/card HTTP behavior still pass. The cumulative delivered_by query/edits, unreadable-scan refusal, unknown-cell handling, replay recipient repair and their four adopted regressions are unchanged by this delta and pass again at the final SHA.

**Cost:** no new loop, timer, poll or process launch. Bulk cards add zero delivery queries; singular state adds one indexed delivery read bounded to 48 rows. The pre-existing one gh launch per delivered call is unaffected. No claim is made about live deployment measurements; performance owns those receipts.

## Nonblocking qualification: bounded card summary

The 48-row budget is a row window, not a guarantee of three complete PR records for arbitrary-sized deliveries. A newest PR with more than 48 addresses can occupy the entire window, yielding a partial address list and fewer than three PR notes. The backing delivery records are not deleted or changed.

This is not a merge blocker for the explicitly requested cheap card summary, but consumers should not treat it as a complete delivery ledger. A follow-up can say “up to three PRs from the newest 48 delivery rows” in the API wording, expose truncation where useful, and add a 48/49-row boundary fixture. The current docs already name the row cap; their opening “newest three PRs” should be read with that qualification. No larger or unbounded query is requested.

## Exact-head CI

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42146651607

Waited for this run to finish successfully before issuing the verdict. All four jobs are green at the exact reviewed SHA: Node 22, Node 24, rust-lint and rust-test. Rust nextest summary: **1,914 passed, five skipped**.

The published `junit-linux` artifact confirms passes for:

- `the_pm_card_read_walks_its_index_without_sorting_the_history`;
- `deliveries_record_once_release_named_claims_and_reach_the_pm_card`;
- `delivered_records_once_prints_edits_and_posts_on_the_pm_card`;
- all four adopted reviewer HTTP regressions;
- `the_card_wire_shape_matches_the_committed_golden`.

CI gate steps also passed fmt, clippy, locked metadata, doctests, both Node versions' typecheck/lint/tests and native contracts. `ddocs-check` explicitly reports UNAVAILABLE on hosted CI, per C20; it did not validate documents there. This delta changes API prose and source, not project DD sources.

Evidence commands were GitHub reads/wait/download only: `gh run watch 42146651607 --exit-status --interval 30`, `gh run view ... --json headSha,status,conclusion,jobs`, the rust job-log API and the published JUnit artifact. Log and JUnit are two views of the same CI origin. No independent local mutation/private-daemon proof is implied.

Final pin inspection: GitHub and the clean detached worktree both remain at `93ca2b1`; PR merge state is CLEAN. No product edits, commits, pushes or worktree HEAD movement.

## Handover and harness gift

No correctness changes are owed at this head. The late card/index change is within the PM's authorized scope. The previous old-head exclusion is superseded by this final-head review.

**magicWand:** preserve the new query-plan guard and add a bounded-data fixture that makes the 48-row card-summary boundary explicit. That locks in cheap bulk reads without silently promising an unbounded ledger.

**difficulties:** none required a local workaround. The CI-only protocol supplied the final index-plan and carried regression receipts while respecting the fleet backoff.
