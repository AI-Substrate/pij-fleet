# Bootstrap — stand up an o-prime

**Store-native is the ruled default**: load [`store-native.md`](./store-native.md) first. It replaces prose portfolio/roster facts, NOT §3's document branch; existing prose stays live until its prime self-migrates.
Use the seating ritual only after authoritative rs designation/event receipts and government/human evidence establish no current prime for this repo, and `pij project list --json` names no project here. `pij list --json` is unfiltered; `--prime`/`--archived` refuse, and display role is not designation. List `--here` scopes to caller cwd and can hide stream worktrees. The absence of a dedicated prime getter is a named gap: never infer permission from it. A missing local `.harness/government/` proves nothing. Existing primes may use §3 without reseating.
Preconditions: git repo, tmux, `pij`, ambient `harness`, one o-prime session, and human-named work; never invent portfolio items.

## 1. Seat and prove the o-prime

1. Load [`../orient-oprime.md`](../orient-oprime.md); if governed from above, run [`kickoff.md#canary`](./kickoff.md#canary)'s three-leg canary.
2. Run the orient's boot audit: channel, dead descriptors, inherited government, human status channel. OMP and Pi each register through their runtime extension at boot; external seats adopt as the orient directs.
3. **Refuse to seat over a living prime**: inspect unfiltered rs roster, current designation receipts/events, project records and authoritative government. If absence cannot be established, ask the owning prime/human rather than bootstrap by inference. An asserted `role: prime` is not designation authority.
4. Resolve your exact id with `pij whoami --json`, then persist `pij orchestration prime set <resolved-seat-id> --json`. Check the returned designation and seq, plus `pij spine events --peer <resolved-seat-id> --json`; do not invent `prime:true`/`oldPrime` list fields. Ambiguous self is failure, never permission to target `operator`.

## 2. Derive the per-repo contract

Inspect the repo; derive these answers, never copy a worked example:

| Contract | Derive |
|---|---|
| Proof + hygiene | Cheap/full deterministic gates; generated/local never-stage paths; CLI-only flow writers |
| Isolation + synchronization | Worktree root/naming; notify-only local actions; shared-resource/convergence batons with a "free" probe |
| Landing | Approved base/SHA; branch push/PR/CI/merge surface (`/builder 8 ship`) |
| Fleet + human | Cheapest proved harness/model; safe ceremony worker; self-identified digest destination |

## 3. Scaffold the government

### Governance branch — the rules

1. **Orphan + permanent**: `prime-governance` is an ORPHAN branch in a standing, permanent worktree. The prime alone commits and pushes there all day; no PRs, no CI, NEVER merge it to main. Normal commit attribution still applies (`00-routing.md` § C11). Main keeps only a pointing `.harness/government/README.md` stub.
2. **Documents move, product stays**: use the table below. This separates document history; it does NOT undo platform-store authority for projects, assignments, events, or seat truth ([store-native](./store-native.md)). Do not recreate migrated facts as competing prose truth.
3. **Receipt-only PRD reconciliation**: check a requirement row ONLY with a receipt (sha / test / run id), never prose. Every plan closes against the named requirement rows it advances. Name the reconciliation owner in local governance: the current o-prime is responsible unless a named successor is recorded; transfer that responsibility at seat rotation. No merge means no automatic PRD review.
4. **Provenance**: lynx (`pij-instant-lynx`), Flowspace3 ruling `2026-08-30-prime-governance-branch.md`; Jordan's 2026-08-30 GO ("commit and push there all day, every day") and 2026-09-07 decision: "encode it".

| Moves to `prime-governance` | Stays on main |
|---|---|
| `.harness/government/**`; retros (including `.harness/records/retro/**`); `docs/plans/prd/**`; prime dossiers | `docs/plans/<ord>-<slug>/` (code PR + the-flow archival); anything CI/gates read; product code and docs |

**Resolver**: in `git -C "<repo>" worktree list --porcelain`, find the record with `branch refs/heads/prime-governance`; `<government-root>` is its absolute `worktree` path + `/.harness/government`. Refuse absent/unavailable worktrees and follow this bootstrap; NEVER fall back to main or a stale local tree.

1. **Discover before creating**: reuse the registered standing worktree. If the branch exists but no worktree is registered, restore with `git -C "<repo>" worktree add "<standing-worktree>" prime-governance`. An unavailable registered worktree needs explicit repair/restoration, not a second tree. A non-orphan existing branch needs a prime-owned, history-preserving re-root; never silently rewrite it.
2. **Fresh branch only**: choose an unused permanent path outside product/stream worktrees, then `git -C "<repo>" worktree add --orphan -b prime-governance "<standing-worktree>"`. Seed the document tree below; commit named paths there with attribution and push `prime-governance` to the approved remote. The first commit establishes the branch ref.
3. **Seed or move documents** using the table; preserve existing content/history before removing any source. Store-native primes create briefs/canaries/local orient and PRD/dossiers as needed, not duplicate portfolio facts. Prose-governed primes instantiate [`spine`](../templates/spine.md) and [`baton-book`](../templates/baton-book.md), plus `briefs/` and `canaries/`; `reports/` only for a real layer above. Every document path is relative to `<government-root>`.
4. **Prose portfolio only**: `harness flow create prime-flow --slug <project>-portfolio --schema <skill-root>/references/prime/prime-flow.schema.json --path <government-root>/prime-flow.json --agent o-prime --bare`. Record existing workshops as inputs; add successors before predecessors. Node status is concurrent truth; `nav.now` is attention.
5. **Main stub**: through the normal code-review path, keep `.harness/government/README.md` pointing at `prime-governance`, its standing-worktree resolver, and this rules block. Never copy live ledgers back; gate inputs stay on main.

**pij worked example, not a universal dependency**: `AI-Substrate/pij` uses `$HOME/pi-hacking/pij-worktrees/pij-governance`; its executable resolver is `harness/scripts/government-root.sh [REPO]` (REPO defaults to cwd). Other repos use the generic resolver above or their own equivalent.

## 4. Install the orient stack

- Keep portable levers [`../orient-oprime.md`](../orient-oprime.md) and [`../orient-global.md`](../orient-global.md) in this skill; never fork them.
- Generate `<government-root>/orient-local.md` from [`../templates/orient-local.md`](../templates/orient-local.md): real product pillars, doctrine, gates, repo mechanics, mandatory reads, portfolio, named PRD reconciliation owner. Omitting a product pillar once made a neat but strategically wrong orient.
- Instantiate [`../templates/stream-brief.md`](../templates/stream-brief.md) per item; specifics belong there, not the portable levers.

## 5. Stand up your PA and close your supervision graph

**A cheap PA is a bootstrap deliverable, not optional context** (Jordan, 2026-08-01; vrell's first bootstrap omitted it). Follow the maintained recipe: `<government-root>/briefs/pa-standup-recipe.md`, resolving the **pij repo's** government root per §3, not the consuming repo's. Do not substitute this summary for that recipe.

1. Cheap tier is the design intent (`gemini-3.6-flash` Copilot seats); whether cheap models hold the rules during chores is still an **open experiment**.
2. **Name the rs supervision gap before declaring the PA ready**: `pij watchdog` and `pij link` are unported; no old watch/bounds/sidecar command is a fallback. Do not claim a subscription or inspect legacy watchdog files as rs authority. Escalate the unavailable mechanism to the prime while continuing independent authorized work.
3. Create/adopt the PA with its supported recorded parent, then the parent asserts `pij role <pa-id> pa --json`. Role alone does not create supervision or grant control authority. Existing-seat reparenting remains unsupported.
4. Keep any separately authorized capture bounded and verify its actual content at the receiver; no claimed watchdog bounds unless the real capability exists. Model and supervision assumptions remain explicit, not production proof.
5. **Prove delivery at the receiving end**, not just configuration.

## 6. Open intake and govern

1. Human-named work enters the authoritative portfolio as `proposed` or `deciding`.
2. At `preparing`, reserve ordinal/folder/window/worktree and branch/base; persist touch set/convergence risks before delegating to [`kickoff.md`](./kickoff.md). **Kickoff is the sole construction owner** for streams, not the permanent governance worktree in §3.
3. Assignment stays provisional through `adopt → orient → preamble`; mark `in_flight` only after its report.
4. Verify then relay; serialize batons, route cross-stream asks, update rows before prose, graduate recurring friction into encodings. Reconcile close-out receipts under §3's rules.

## Recovery

| Event | Recovery |
|---|---|
| Seat death/restart | Fresh seat loads lever 0 + government; audit dead holders, stale roster, orphan descriptors; record new identity |
| Ruled rotation | Outgoing writes [`../templates/seat-handover.md`](../templates/seat-handover.md) BEFORE contact; incoming transfers writers/reconciliation owner, checks spine Seq, retires outgoing after final send |
| Queued sends never deliver | Inspect actual rs receipt/outcome and daemon health; do not sweep legacy descriptors or withdraw in-transit answers |
| Stream dies / is retired | Use current rs binding/tombstone evidence and explicit native revive/spawn; kickoff owns record retirement and separately authorized preservation/teardown |
| Fence gap in verified worktree | Stream persists and notifies; o-prime records overlap; continue unless hard ownership or convergence boundary |
