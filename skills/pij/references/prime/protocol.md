# o-prime protocol

A governed way to run many agents in one repository: one **o-prime seat** owns
coordination, streams own plans, fleets do the work. Durable store records and
single-writer documents make replacing a seat a read, not a handover conversation.

## Roles

| Role | Cardinality | Owns | Never does |
|---|---|---|---|
| Human | one or more | Names work, gives rulings, final authority | Delegates final authority to protocol |
| o-prime | one seat per government | Portfolio, allocation, fences, batons, roster, rulings, verification, digest | Stream implementation or a stream's plan mutations |
| Stream orchestrator | one per in-flight work item | One plan end-to-end and its worker fleet | Crosses hard ownership boundaries or uses shared/converging state without synchronization |
| Fleet worker | many, bounded | One packet inside a stream's narrowed allowlist | Governs siblings or expands scope |
| Optional overseer | zero or one | Audits the o-prime and receives numbered reports when explicitly installed | Becomes a required structural layer |

Government is durable substrate, not a role; store authority and document ownership are distinct.

## Hierarchy and escalation

```text
human
└─ [optional audit layer]
   └─ o-prime
      ├─ stream ─ fleet workers
      └─ stream ─ fleet workers
```

Escalate governance, coordination, and blocked-work state exactly one hop:
fleet → stream → o-prime → human, inserting the optional audit layer only when
it actually exists. Streams never negotiate sideways; cross-stream needs go
through the o-prime. Escalation never transfers a context-local question — its
owner asks the human directly (§ Human rulings). A human may enter any pane.

## Government files

The o-prime alone writes documents under `<government-root>`; resolve it via
[Governance branch — the rules](./rituals/bootstrap.md#governance-branch--the-rules).
Briefs, canaries, local orient, rulings and encode candidates live there.
For unmigrated prose governments, so do `spine.md` (thesis/roster/fences/watch/
allocations/rulings), `baton-book.md` (leases/queue/grant log), and `prime-flow.json`.
[Store-native](./rituals/store-native.md) governs migrated facts; never duplicate
store authority in prose. Document location does not change who owns those facts.

Anyone may read. Update rows before prose, stamp mutations in UTC, strike or
tombstone history rather than erase it. A hard restart lost no government state:
that is the test. Shapes: [spine](./templates/spine.md), [baton book](./templates/baton-book.md).

## Portfolio and lifecycle

Portfolio items move `proposed → deciding → preparing → in_flight → done | folded |
dropped`, plus `blocked`; node status is concurrent truth, `nav.now` attention.
Orchestrators move `adopt → orient → preamble → work`: prove channel, enter via
`/pij prime`, load orient stack + `/thesis`, persist the human preamble report
before work. Validated plans stop at `WAITING_FOR_BUILD_CONFIG` until the human
confirms the fleet. [Kickoff](./rituals/kickoff.md) owns construction/teardown;
close every plan against named PRD rows under the governance rules above.

## Construction, fences, batons, and landing

The worktree-primary construction path gives each stream one recorded branch and
working tree based on an approved SHA. Create and verify it before spawn; run peer
spawn from that cwd so descriptor and pane inherit it.

A fence is descriptive ownership plus expected merge-risk metadata — a sensor
that informs, never a gate that blocks. Work confined to the verified
worktree/branch — reads, edits, hermetic tests/builds, commits, sole-owner
branch pushes — is notify-only.
A newly discovered worktree-local path is a **tell**: persist and report it;
separate-branch overlap is notify-now, reconcile-at-convergence. Hard ownership
rules remain hard: notification never permits writing the o-prime's government,
another stream's worktree, or CLI-only flow state.

Isolation removes edit-time serialization, not convergence-time serialization.
Synchronize before histories or mutable state converge: same branch or shared
checkout/index; merge, rebase, or landing to a shared target; consuming a moving
branch; or any shared mutable resource concurrent use can corrupt. Batons are
interlocks — one holder, real hazards only. Trigger matrix, edge cases, and
lifecycle: [`rituals/batons.md`](./rituals/batons.md).

Approved work lands through `/builder 8 ship`: confirm-gated branch push, PR open,
watched CI, then optional typed-confirm merge. Remove the worktree only after PR
merge or explicit abandonment. In shared-tree fallback, scratch/staging,
pathspec commits, staged-set checks, and commit slots remain mandatory. Apply `00-routing.md` § C11 (Commit attribution) when creating commits.

## Reports and verification

Report files keep this shape:

| Field | Meaning |
|---|---|
| `claim` | exact outcome claimed |
| `artifacts[]` | durable evidence paths |
| `shas[]` | commit/content hashes |
| `gates[]` | command, verdict, output path |
| `observations[]` | portable lessons and suggested encodings |
| `open[]` | unresolved decisions, risks, skips |

Receivers verify one load-bearing artifact or cheap gate before acting/relaying;
verification is itself a claim. Freeze/hash mutable targets ([reports](./rituals/reports.md)).
Without an upper layer, store records + government documents are the evidence;
humans get main-event digests. Numbered reports require a real optional audit layer.

Wire traffic around reports follows § C10 (`00-routing.md` § Shared
conventions): a clean verification **of a received claim** sends nothing back
to the sender — silence after a clean verify is the all-clear, and unsolicited
confirmations are the fleet's measured top waste. Contract-mandated reports
(preamble, phase checkpoints, ship) still file and still send their pointer. A governor enforces C10's exception as firmly as its terseness: a
correction of a false belief carries its full reasoning (a bare "no" gets acked,
not internalised), and a reviewer should treat a reason-less correction as a
defect.

## Fleet messaging: every send costs the recipient's context

Measured (RCA by pij-continued-bolvar, 2026-09-28, workspace `~/games/unasphere/scratch/usage-blowout/`): a week's Claude allowance went in 38.6 h. Turns opened by pij messages were 84% of spend (6,103 messages against 1,025 human turns). Warm cache reads on huge contexts were 64% (a mean of 336k tokens per call). Acks and FYIs alone were 15%, and cold wakes 10%. Primes were the hubs.

1. **Price a send at the recipient's context, not the message.** A turn costs the recipient's whole context: warm, about 1×; cold, about 40×. Waking a large seat (over 300k) that has been idle for more than an hour needs a reason you would defend to Jordan. The daemon enforces this: such a send is refused `E-RS-COLD-WAKE` with its price until you hold it with `--fyi` or pass `--force --reason '<why>'`, which is audited (see the peer route).
2. **Broadcast only to seats it changes.** A multi-seat send goes only to seats whose next work it materially changes, and that are either working now or warm (last call under an hour ago, so no cache re-read). Leave out a recently idle seat that won't resume work, and any large cold seat. Name the recipients explicitly, filtered from `pij list --json` and `pij state`. A freeze ("hold commits") goes to the active committers only.
3. **No acks, no turn-opening FYIs, no-reply by default.** Use `--fyi` (`pij send <seat> --fyi '…'`, held until the seat's next turn) only when the recipient's next action doesn't depend on it; questions, blockers, hand-offs, verdicts and work done are normal sends, and what they'd be fine never reading is not sent at all ([the FYI rule](../routes/peer.md#converse)). Batch what one seat needs into one message rather than several. Silence after a clean verify is the all-clear (C10).
4. **Standing rules and status live in files.** Put rules in government files (the governance README, AGENTS.md), and read status from report files and `pij state` rather than asking for push reports.
5. **A dormant seat is consulted, never informed.** Wake one or two only for their knowledge (the original owner, a reviewer).
6. **Supervise by observation.** Read tmux captures and `pij list`; never message an idle seat to check on it. Stop after two quiet rounds.
7. **Compact before going cold, but only if you'll be needed again.** When a prime ends a unit of work above 300k and expects more work (an open stream, a pending ruling, a coming phase), it compacts itself at that boundary while its cache is still warm. A seat that is finished for good is not compacted: a compaction nobody reads is pure spend. A compaction more than an hour after the seat's last call re-reads the whole history uncached, at full cost. A cold seat compacts only when it can justify that cost. Compaction is always the seat's own choice. Nobody forces it on another seat, and no guard triggers it.

Platform guards that will enforce this are tracked in issue #446.

## Seat identity

A pij id names a **seat**, not whichever persona currently speaks in its pane.
Role-address sends when contexts can differ; replies declare the speaking role.
No agent message is consent; a relayed ruling binds nothing until the owning
layer or human confirms it. An orchestrator seat never runs a long blocking
subagent in its own session — spawn peers; one seat went deaf for hours when
its go-signals landed in a research context that refused them.

Windows: o-prime `o-prime` · stream `s<ordinal>-<short-slug>` · fleet panes
inside the stream's window. Names aid humans; identity remains the registry id
and pane binding.

## Human rulings and non-blocking questions

Humans outrank every channel. Record their words immediately, preferably
verbatim, in durable committable government or plan files — never only in chat
or scratch. If a direct human go may collide with sibling work, notify the
o-prime before execution unless the human says now-regardless.

Human attention is the scarcest shared resource in the system; the prime is
its scheduler, and orchestration must remain live while the human is absent or
remote. No seat or peer ever uses `ask_user_question` or any modal question UI
— a modal wait serializes the human: an outage. Ask inline, persist a pending
decision (spine § Pending decisions), block only dependent work, batch related
questions, digest at the human's cadence, continue the rest.

Question ownership follows working context: the agent that will use the
answer asks the human directly (Builder asks its own planning questions);
its parent receives a pointer but never paraphrases, pre-answers, or asks on
its behalf. The o-prime asks only portfolio/government questions it owns. If
the owner lacks a direct human channel, its parent forwards the persisted
question verbatim by pointer and routes the answer back.

## Canary and cold readers

Every spawn or adoption is canaried before brief, recursively, with the record
written at pass time; mechanical identity beats self-report. Ritual and labeled
history: [`rituals/kickoff.md`](./rituals/kickoff.md),
[`exemplars/canary-record.md`](./exemplars/canary-record.md). Schedule
fresh-reader audits at adoption and checkpoints — cold readers found stale
government and false completion claims that warm authors read past.

## Portability and orient levers

Levers 0 [o-prime](./orient-oprime.md) and 1 [global](./orient-global.md) stay
portable/authoritative; derive lever 2 from [local orient](./templates/orient-local.md)
and put item specifics in one [stream brief](./templates/stream-brief.md).

Repo-local configuration derives cheap/full gates, batons plus free probes,
never-stage paths, flow-writer rules, fleet defaults, human digest channel, and
ceremony tier. Nothing repo-specific enters the portable levers or this file.

## The second objective

Every layer completes the work and improves the environment it ran through:
capture friction when it bites, carry observations upward, graduate lessons
local orient → portable orient/protocol → tooling. Encode, don't document.
