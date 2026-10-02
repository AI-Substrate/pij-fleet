# Batons — serialize exclusive resources

The rs primitive is `pij orchestration baton <verb>`: one holder in the shared SQLite lease store,
with purpose-bearing requests, pin re-verification and attributed spine events, not legacy lease files.
The **book is annotation/evidence, not lease authority**: one o-prime writes judgment; no primitive writes it.
See the [stable API](../../../../../docs/how/pij-rs-api.md#baton-leases) and start from the
[`baton-book` template](../templates/baton-book.md).

Serialize real hazards: shared build locks, ports/services/daemons, global package/config/cache/runtime
state, rate-limited APIs/accounts, shared fixtures/generated artifacts, same-branch/shared-checkout work,
moving-branch handoffs, rebase/landing/merge, and git/index use during a ruled shared-tree fallback.
**Isolation removes edit-time serialization, not convergence-time serialization.** Routine reads, edits,
hermetic tests/builds, commits and sole-owner pushes in verified stream-owned worktrees/branches are
notify-only: **do not request a baton**. Isolated branches touching the same path record overlap now and
synchronize at reconciliation. Immutable producer-SHA consumption needs no baton until repinning;
moving-branch consumption needs a handoff baton. Unique-branch pushes are notify-only unless CI/external
quota is shared; merging to a shared target is always serialized.

## Lifecycle (ritual step → primitive verb)

1. **Define** once: `pij orchestration baton define <name> --resource <text> [--probe <cmd>] [--repo <path>]`.
2. **Request**: `pij orchestration baton request <name> --purpose <text>`; optionally add `--pin <sha>`
   and `--evidence <declared return evidence>`. Preserve the receipt; a queue position is not a promise.
   The service persists the request and publishes `baton.requested`, then calls existing DeliveryService
   from requester to the definition's `created_by` keeper. Inspect `{request, seq, notice}`; do not send
   an unconditional duplicate notice. `queued` is admission (OMP and Pi each await ReaderRead), not observation;
   `delivered` requires transport confirmation, not proof of keeper observation. `unverified` uses
   Dead/Recycled process-incarnation evidence, not heartbeat age; missing/dissolved keeper gives null/no send.
   Publication failure prevents send. Send/probe errors or Held/Refused outcomes cannot erase commitment
   or masquerade as success: `E-RS-PARTIAL` retains `committed`, `event_published`, `request` and `seq` evidence.
   Later reader acknowledgement needs no second notice; self-send retains the existing refusal path.
3. **Verify free** using the resource probe and holder liveness; after restart, never trust tables alone.
4. **Grant**: `pij orchestration baton grant <name> --to <request-id> --json`; preserve its lease id.
   Grant checks pin against current HEAD: stale/unverifiable requires explicit `--repin` after inspecting
   real evidence. Keeper annotates the book and communicates the receipt; acceptance is not recipient receipt.
5. **Use** only for the recorded purpose. Negotiated sibling windows must be recorded in the book;
   the primitive has no sub-leases.
6. **Return**: `pij orchestration baton return <name> --lease-id <observed-lease-id> --evidence <text>`.
   Use the exact grant/show lease. State/event commit atomically; keeper verifies declared evidence
   before closing the book row. Evidence verification remains human.
7. **Reclaim explicitly**: `pij orchestration baton reclaim <name> --lease-id <observed-lease-id> --evidence <text>`.
   Re-read `pij orchestration baton show <name> --json` before judgment. **Never reclaim from silence
   alone**; no daemon auto-reclaim is implied.

`pij orchestration baton list --json` / `show <name> --json` expose definitions, current lease and requests.
Missing/mismatched `--lease-id` refuses `E-RS-LEASE-STALE`, naming current lease and remediation;
a shim must never resolve a newer lease for an old caller.

## Hard paths — the primitive records, never decides

Reclaim/breach examples: [`grant-log`](../exemplars/grant-log.md).
- **Self-grant**: keeper requests, verifies, logs, uses and returns through exactly the same path; no shortcut.
- **Silent holder**: inspect current process/pane and durable lease evidence, then ask the keeper;
  silence is neither completion nor permission. On restart audit book + list; explicitly reclaim dead holders.
- **Queued posture**: pre-stage shared-tree fallback batches in scratch; land inside the granted window.
  For timing/external batons, prepare all non-contending inputs while waiting.
- **Fallback git-index**: docs edits during another holder's window stay unstaged and disclosed;
  `git add -A` can sweep sibling files into the wrong commit (INC-004).
- **Breach**: stop competing use, tell the holder, record it, then fix the inviting path; record is enforcement.
- **Timing/contention**: preserve actual request/grant times and current lease receipts; no invented blocked-time
  projection. Persistent contention informs a sensor, resource split or human sequencing ruling.

### Fences: sensors, not permission

Fences record merge risk, never block; batons interlock real shared-state/convergence hazards.
Neither gates isolated branch edits. Answering in-worktree “may I?” with `GRANTED`/`REFUSED` teaches
permission regardless of doctrine; unowned defects and ask-and-wait loops are tells. A lifecycle hook
escaping the worktree warrants serialization because of shared state, not because of a fence.
**A path fence bounds writes, not blast radius**: an outside snapshot can assert in-fence bytes and fail CI
while every write stays in-fence. **If enforcing a fence, check the status layer first**: fences predict
intent; cards report actuality. Rotten cards make enforced predictions substitute for evidence.
The stale-badge failure split write/clear operations so holders could not see their own stale state.
Restore legibility: “tell me what you're touching and I'll record merge risk.” A touch-set is a NOTICE.

### Cause and expiry — controls, not memory

A stand-off can be justified initially by real competing writers yet rot after the other seat closes.
The defect is that nothing expires the constraint when its cause dies, not necessarily its initial issue.
**Every durable constraint must state its cause and expiry condition together.** Example: “stood off X
BECAUSE araminta holds FU-4; EXPIRES when araminta releases it or is closed.” Closing becomes a visible trigger.
The same accountability gap affected stale badges, unread companion baselines (companions cannot see ack),
fences after seat closure and PA exemptions after their reason passed. Split write/clear operations are
its mechanism; stateless companions remove one class, not otherwise-valid state whose justification died.
**Positive control**: interim guidance named holding defect `pij#72` because the debt-not-practice rule
required expiry. It held because a rule required it, not because its author remembered to be careful.
**Negative control**: koala already had a personal stale-badge correction note from a previous failure,
yet produced 300 minutes of contradictory state. Knowing the rule was not a control, even after prior harm.
Rule-enforced expiry held; remembered expiry failed in the same fleet on the same day. The badge must
expire itself or be cleared by the operation resolving its cause: fix the write path, not anyone's memory.
