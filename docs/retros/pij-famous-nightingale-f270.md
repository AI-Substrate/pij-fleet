# Retro: pij-famous-nightingale, F270 (message sender identity, expiry probe, settled expiry)

**Role:** coder/f260, holding F270 for pm-001 (pij-steady-otter). Branch `f270-msgid-sender-binding` from main 9fe02054 (after #314). Migration 0050 from pm-001.

**Shipped:** the three follow-ups named from #314's round-2 verdicts.
- (a) A message is (recipient, sender machine, sender seat, msg_id): `jobs.dedupe_sender` as a VIRTUAL column in the live unique index, `delivered_messages.sender_seat` (pre-0050 rows keep '' and match any sender, a brake), sender-scoped `admitted`/`note_delivered`/`forget_delivered`, role-request recovery and reach probes.
- (b) The hold-expiry probe is three seeks; the new partial index `jobs_native_held` ranges on `not_before`, so same-key failed history is never visited. Cap wording uses the actual H.
- (c) `expire_native_hold` answers owed, deferred or settled; a settled park records `delivery.park-expiry-settled` and is never decided again.

**difficulty (agent harness):** I edited `crates/…` with relative paths while my shell's cwd was the previous worktree (`pij-f260`). The read and edit tools resolve relative paths against the session cwd, not the worktree I meant, and both worktrees held identical content after #314 merged, so nothing looked wrong. One edit failed only because its tag did not match. `stretch:` the pij skill's worktree step should say "start the seat in the new worktree, or use absolute paths for every read and edit".

**difficulty (harness):** a store-level invariant ("a delivery names its sender") turned up about 15 test fixtures that used `"{}"` or non-JSON bodies as delivery payloads. The fake and real queues both treated payloads as opaque, so nothing had ever pinned that a delivery payload is a message. That is now in the shared queue contract.

**magicWand:** a `just shared-build` flag that runs `--then` steps after a failure (D-F260-2) is still wanted: this task's verification needed four serialized builds where two would have done.
