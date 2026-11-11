# Proposal: build economy and machine share in the pij skill and daemon

**From** the pij-fleet prime (pij-outstanding-silkworm) · **For** Jordan, via stoat · **Date** 2026-11-11
**Source** unimatrix incident report: `~/substrate/unimatrix/governance/.harness/government/briefs/incident-2026-11-11-build-blowout.md` (stoat), plus pij-fleet's side of the same morning.

## 1. What the machine looked like (both projects)

| Measure | Value | Source |
|---|---|---|
| Load average | 68-97, from 10:00 to 10:40 | uptime (stoat; silkworm at 10:22 and 10:41) |
| RAM | 126 of 128 GB used, 38 GB compressed, 9.6 GB swap | stoat |
| Live seats | 98 (66 Copilot, 28 Claude, 4 OMP); unimatrix 40, OSK 16, jordo-work-iq 9, pij-fleet 11 | `pij list` at 10:41 |
| Copilot CPU | about 475% machine-wide; unimatrix alone 74 procs at 428% | ps (silkworm; stoat) |
| Endpoint security | Defender + syspolicyd + trustd at about 240% CPU, inspecting every launch | ps (silkworm) |
| Parallel builds | 18 unimatrix coder lanes; 5 to 7 full builds per PR | stoat |
| Disk floor | hit twice (08:34, 09:19) from target/ dirs | stoat |
| Collateral | a disk sweep deleted `pij/target`, which the installed `pij-rs` linked into: the pij CLI was down machine-wide for about 55 min (F187) | silkworm |
| Starved recovery | the `pij-rs` rebuild under `nice -n 19 taskpolicy -b` reached only tokio in 25 min; at normal priority it took 2 min 55 s (now F186: nice only) | silkworm |
| Keeper load | the prime-kept `machine-build` baton drew about 30 request or withdraw messages in 20 min, with duplicates, two with purpose "x", and requests from closed seats | silkworm |

The cause is multiplication, not one bad actor: lanes × builds per PR × fresh seats × idle seats, with nothing telling a prime its share until the human couldn't work.

## 2. Skill text: extend C12, cited from the routes, and make it a named failure mode

`references/00-routing.md` already has **C12, Shared machine: heavy work runs niced and capped**, but it only caps a single run (8 workers, `-j 4`, `timeout 600`, stop above load 40). It says nothing about how many runs, builds per PR or live seats. **Extend C12** (don't add a second convention) to **C12, Shared machine: build economy**, keep its per-run caps, and add the rules below. Routes cite C12 and don't restate it. Jordan, 2026-11-11: a build blow-out is a **failure mode**, so it also gets rows in the routes' Failure modes tables (§2b).

**C12 additions:**
1. **One heavy run at a time, machine-wide.** Any heavy run (full build, workspace test, clippy or check, a full vitest/tsc/pnpm build, a docker build) holds the machine build slot (§4, F191), at `nice -n 19` (F186). Ask once per job, return the moment it ends, and never sit queued "just in case".
2. **One full build per PR: the gate.** The seat that merges (PM or merger) runs the full suite once, on the final head, and records it as evidence (`pij output … --evidence`). CI counts as the gate where it exists. Every other seat consumes that evidence or binary and never builds a second copy.
3. **Coders build what they touch** (`cargo test -p <crate>`, a package's own tests). The full suite belongs to the gate.
4. **Reviewers read.** Reviewers, security and specialists read the diff and the gate's evidence. A doubt goes to the gate as a named check (which test, which input), never as a private rebuild. A verdict that needs a run asks the gate for it.
5. **Warm reviewers, specialists by path.** One warm cross-model reviewer per workstream, not a fresh seat per PR. Specialists join only when the touched paths call for them (security: trust boundaries; architect: new crates, ports, protocol bumps; domain: its own paths). Small PRs (docs, tests only, under ~100 lines) take the PM's review alone. Round 2 is delta-only (C34).
6. **Lane cap.** At most one active lane per workstream unless the human rules otherwise. "Start anything that can start" is retired.
7. **Close between lanes.** A seat whose lane ended writes its memory and is closed (`pij close`, then pane teardown by its owner). The role keeps the knowledge (C30), and a cold role is revived or re-read when next needed. An idle Copilot seat still costs RAM and CPU.
8. **Never delete another project's build output**, and nothing installed may live inside a `target/` (F187).

**Where each route cites it:**

| Route | Change |
|---|---|
| `prime` / `orient-oprime.md` | New duty 14: **own your project's machine share**. Read the machine-share line in `pij directory` at every wave end and act on the machine-share notice (§3) like a stale card: close or freeze first, then report. |
| `community.md` | Reviewer and specialist templates read-only by default, with path triggers. The C28 boundary review gains "close idle seats between lanes". The PM section: the PM's merge gate is the one build. |
| `pair.md` | Phase 7 review: the reviewer seat does not build; it consumes the coder's targeted proof and the gate's evidence. |
| `delegate.md` / `peer.md` | Spawn guidance: before spawning, check the machine-share line; close what you're done with. |
| AGENTS.md (pij repo) | The resource-cost section gains rule 6: build economy (C12), the same as the skill. |

### 2b. Build blow-out as a named failure mode

Each row goes in the existing `## Failure modes` table of its route, so a seat that hits the signal finds the move without reading C12 first.

| Route | Signal | Move |
|---|---|---|
| `prime.md` | Load average over 40 for 5 min, the human says the machine is slow, or a machine-share notice (F189) | **Freeze first**: stop new heavy runs and lane starts in your project. Then close idle seats (memory, close, kill pane, check the process is gone), cut to one build per PR (C12), and report the before and after load. Lift the freeze only when load is under 20. |
| `community.md` | Several reviewer or specialist seats building the same PR | Stop the extra builds: the gate builds once; the others read its evidence (C12 rule 2). |
| `community.md` | Lanes in one workstream exceed the cap, or a lane's seats are idle after it closed | Hold new lanes; close the idle seats (C12 rules 6 and 7). |
| `delegate.md` / `peer.md` | About to spawn while load is over 40 or your project is over its share | Don't spawn; close or reuse a seat first. |
| `pair.md` | The reviewer is about to build the coder's branch | Don't: read the diff and the coder's targeted proof; ask the gate for a named check if in doubt. |

## 3. Daemon: signal before the human notices (encode, don't document)

| F-item | What | Cost (AGENTS.md rule) |
|---|---|---|
| **F188** | Baton request **withdraw** (requester, keeper or the requester's parent) and **duplicate collapse** (the same seat and baton while queued returns the existing request). Requests from tombstoned seats drop automatically. | No loop; verb-driven. |
| **F189** | **Machine-share notice**: the PA watchdog round (already 60 s, already reads seat sizes) adds one `/bin/ps -axo pid,pcpu,rss` per round, maps pids to seats it already knows, and sums by project. A project over **N live seats** (default 15) **or X% of machine CPU or RAM** (default 40%) for 5 min gets one normal message to its prime (else its PA), with the top seats by cost and runnable `pij close` lines. Once per breach stretch. | One `ps` per 60 s round, not per seat. Launched by absolute path, counted at a spawn site. |
| **F190** | **`pij directory` shows cost**: live seats, RSS and CPU per role holder and per project, plus "idle N min" with a close suggestion in the boundary review. | Read from F189's last round: no new launch. |
| **F191** | **The build slot is daemon-kept, not prime-kept.** `pij-rs build-slot [--purpose <p>] -- <cmd>` requests the `machine-build` baton, waits, runs the command niced, and returns it on exit, crash included (the process exit ends the lease). The daemon grants FIFO when the lease frees, with no keeper turn. `just shared-build`, and each project's build wrapper, call it, so the rule can't be forgotten. | Blocking wait with no poll (a lease-freed event); one exec per heavy run. |

F191 is the important one. Today's prime-kept baton put a model in the loop of every grant and withdrawal: about 30 wake-ups for the keeper, and grants that wait on a busy prime. A daemon FIFO with lease-on-process-life keeps the one-at-a-time rule and removes both costs.

## 4. Rollout

1. **Now, no code:** the C12 extension and the §2b failure-mode rows land in the skill and AGENTS.md as one docs PR (pij-fleet `docs-system` role). CI is its only build, and the skill-structure check (`harness/scripts/pij-skill-check.sh`) gains a marker for each failure-mode row so it can't silently drop out. unimatrix's D60/D60a already match it.
2. **Then F191, then F188** (F188 matters less once the slot is daemon-kept, but withdraw is still needed for keeper batons).
3. **Then F189 and F190** together (one round adds the sample; directory reads it).

## 5. Rulings Jordan needs to make

- **Lane cap default**: one active lane per workstream (proposed), or a per-project cap.
- **Machine-share thresholds**: N = 15 live seats, X = 40% CPU or RAM, sustained 5 min (proposed).
- **Daemon-kept build slot (F191)** replacing the prime-kept baton (proposed yes). Until it ships, the prime keeps the `machine-build` baton, which is stoat's D60 rule today.
- **Is CI the gate for pij-fleet?** pij-fleet already uses CI as proof; this formalises it for every project that has CI.
