# PR #131 C29 round review

## Verdict

**pass-with-notes**, exact head `599747c6727140f883ccc5708f3f03a7d7f73cc7`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/131#issuecomment-6677129123

No new correctness blocker in the C29 alias/release/lookup change or the three merge resolutions. Prior c6aa07b dispositions stand: original F001–F003/N001 closed, F004 withdrawn; the nonblocking aggregate-provenance note is not re-reviewed or promoted to a new gate.

Scope: feature commit `f0474a8` and the `599747c` main merge, relative to the prior approved feature head. The broader raw diff imports already-merged #132/#140; it is not treated as a request to repeat their full reviews.

CI first. No local test/build/probe was needed. Fresh detached worktree stayed unchanged; no product edits, product commits/pushes or deployment by this reviewer. The authorized role-memory writer's governance commit is recorded below.

## Alias and release correctness

- `core/src/claims.rs` adds three-digit `workstream` and explicitly aliases `roadmap` to it. Other kind names remain unchanged; the distinct `pij stream` concept is not renamed.
- Claim canonicalization happens before checkout-floor calculation and `claim_number`. Both spellings therefore use the same `(project, workstream)` transaction/unique key, not separate counters.
- Both spellings scan the same two known folders, `docs/project/workstreams` and `docs/project/roadmaps`, taking their combined maximum. The existing absent/unreadable distinction and no-allocation-on-error behavior remain intact.
- Manual release canonicalizes before constructing `ClaimKey`, so `claim release roadmap N` releases the canonical workstream claim. Ownership checks and permanently burned released numbers use the unchanged store logic.
- `numbers_in_paths` skips alias entries when iterating known kinds and emits canonical workstream keys for files under either folder. This makes merge-triggered claim release agree with allocation/manual release without duplicate alias entries. Tombstone release remains seat-based and unaffected.
- The new HTTP sequence regression proves 002 after legacy folder 001, 003 through the roadmap alias, 008 after adding workstreams/007, and successful alias release of the canonical workstream row.

## PM lookup

`Address::workstream()` recognizes a `steps` row in either folder, checks the same numbered basename rule and excludes nested extra path components. The unchanged relative-address parser remains in front of it.

`holder_pm` derives the same `pm-NNN` role from either address form, deduplicates role/seat candidates and retains live-holder/fallback semantics. The extended HTTP test proves the legacy folder still selects the holder and that `docs/project/workstreams/001-…#steps/s02` selects that same pm-001 holder rather than losing the notification.

C29's future nesting, schema and coverage work is outside this pre-merge hardcoded-kind correction; its absence is not a finding here. Historical plans/outputs/old folder references need not be rewritten.

## Merge resolutions

Reviewed the combined merge diff and the result against both parents:

- **Services:** `gh` and its resolve-once initializer survive alongside main's `memory_turn_bound` configuration/warning logic and shared `memory_turns` state. Relative to main, this is only the expected gh addition; neither side was dropped.
- **API doc:** role-memory commands, held close/memory-turn contract and refusal documentation from main remain, together with all claim/claims/delivered endpoints and the C29 workstream/roadmap text.
- **Skill table:** main's directory-memory verb/procedure row remains and the separate claim/claims/delivered row includes the alias and workstream PM lookup.

The merged-head CI also exercises held compact/close decoding, memory-turn HTTP behavior and claims HTTP behavior, corroborating coexistence rather than merely compilation of copied text.

## Cost and proof

C29 adds one extra known-folder read to this kind's existing in-process scan; the alias uses the same scan, not two independent allocations. No subprocess, timer, poll or recursive filesystem scan is added by C29. Delivery still has its existing gh read. The inherited #132 memory-turn work is not misattributed as new s11 polling.

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42163082170

All four jobs pass at the exact reviewed SHA: Node 22, Node 24, rust-lint and rust-test. Rust summary: **1,945 passed, five skipped**. Published JUnit confirms:

- `workstream_and_roadmap_claim_one_sequence_across_both_folders`;
- the extended live-holder/no-holder delivery test;
- core canonical path/number and Delivers address tests;
- all four original reviewer HTTP regressions;
- the 60-address completeness and concurrent-claim controls;
- merged memory-turn and held-receipt controls.

These are CI execution receipts, not local or live-fleet measurements. No independent local mutation run was performed. No CI gap justified using the niced fleet-190 slot. Final GitHub/worktree pin matches `599747c`; PR is CLEAN.

## Handover and harness gift

No new correctness change is owed for this round. Previous c6aa07b passes and their explicit nonblocking note stand. Security and performance retain their separately assigned path/cost verdicts.

**magicWand:** test an alias at every identity boundary, not just its printed label: allocation, release, file-derived keys and role lookup. This round now has executable combined-folder/shared-counter and renamed-address holder controls, plus merge-aware CI.

**Memory protocol:** this round's role memory was written through `pij directory memory add` using a real body file, not manual note/INDEX edits or the shim's unsupported stdin form. The writer returned commit `2113b1cc2d05de40c5e4bfd9522f0e2a9f9777b7` and note `memory/2026-11-06-pr-131-c29-alias-and-merged-head-599747c.dd.json`; a `pij directory memory reviewer --note ...` read-back verified the persisted body and metadata. No new F-item arose.
