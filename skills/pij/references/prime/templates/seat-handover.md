# Seat handover — <outgoing pij id> → incoming o-prime
**Written**: <ISO> · **Trigger**: <ruling pointer — rotation is ruled, never self-invented>

> ORDERING CONTRACT (first outside rotation hit this race): the outgoing seat
> WRITES this pack and re-stamps/bumps the spine BEFORE the incoming seat is
> told to make contact. An incoming seat whose government read predates the
> pack rebuilds state without the newest rulings.

## Boot path for the incoming seat

1. **Native OMP, Pi and Copilot seats each keep their runtime's extension-owned registration.** Other supported tmux seats adopt only their exact current pane per C1. Resolve incoming identity with `pij whoami --json`, then lever 0.
   Persist `pij orchestration prime set <incoming-pij-id> --json`;
   preserve the designation/seq and corroborate its spine event before changing any writer line. No `list --prime` or tree projection exists; display role is not designation. Until step 5 retires the outgoing marker, the explicitly ruled designation overlap prevents a discovery gap.
2. Read and write this pack under `<government-root>`; resolve it via
   [Governance branch — the rules](../rituals/bootstrap.md#governance-branch--the-rules).
   Read authoritative store facts and remaining unmigrated prose:
   spine → baton book → prime-flow (CLI-only) → briefs → THIS PACK →
   local orient. Check the spine `Seq:` counter against this pack's
   `spine-seq at write:` line — a mismatch means you are reading mid-write.
3. Rotation checklist — transfer EVERY writer line (they are easy to miss):
   - [ ] spine `Writer:` + a rulings entry recording the rotation
   - [ ] baton-book `Writer:`
   - [ ] orient-local writer/tuner line
   - [ ] anything this repo added (grep `Writer:` under the government root)
   - [ ] named PRD reconciliation owner + outstanding plan-to-row receipts
4. Announce to the human and to every live stream **citing this pack** —
   streams do not know your id; say so explicitly per stream. All your A2A
   sends follow § C10 — Wire discipline (`<skill>/references/00-routing.md`
   § Shared conventions).
   4b. Govern **store-native** from here (ruled default): self-migrate
   load-bearing facts lazily as you touch them — the rule and verb mapping
   live in [`../rituals/store-native.md`](../rituals/store-native.md).
5. After the outgoing seat's FINAL send,
   run `pij orchestration prime retire <outgoing-pij-id> --json`.
   Verify its returned `prime.state` is retired and retain the seq;
   read `pij spine events --peer <outgoing-pij-id> --json` for the historical record. No `oldPrime` tree/list field is implied. This retires designation, not the process or pane.

## Live state inherited

- Streams + fleets: <id · plan · phase · fleet ids · what it does NOT know yet>
- Batons: <book is truth; name holds + standing rules>
- Sequencing watches: <ids + the ones with closing windows, flagged>
- Ruled-and-settled: <decisions with DO-NOT-RE-LITIGATE markers>
- **Session-bound dependencies**: <anything whose completion lands only in the
  outgoing seat's session — each carries an explicit relay contract:
  "I relay X verbatim-by-pointer before standing down">
- Uncommitted tree: <paths + WHY uncommitted + whose decision committing is>

## Outgoing-descriptor lifecycle (one rule, no synthesis needed)

The outgoing seat is retired **by the incoming seat, after the outgoing seat's
FINAL send** (all relay contracts discharged, stand-down announced). Its descriptor
remains queryable as old-prime history; any separately ruled pane teardown must
preserve that registry evidence. Track relay, retire, and teardown as sequencing
watches. The pack + stand-down note constitute the owner's explicit ask.

**spine-seq at write**: <spine Seq value when this pack was finished>
