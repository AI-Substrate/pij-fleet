# Skill survey: partition `prime-core`

Files: `skills/pij/references/prime/{orchestrator,orient-global,orient-oprime,protocol,pa}.md` @ main bb9f0b1.
Ground truth: `pij --help`, `pij-rs <verb> --help`, `crates/` source, `harness/scripts/government-root.sh`, `docs/how/pa-watchdog.md` (docs used only as secondary evidence).

## Summary (ranked by severity)

1. **WRONG**: orient-oprime L139-142 says a prime's `status-stale` is "DROPPED by the anomaly sweep… `target === null`" and that the PA closes the gap "by POLLING `pij anomalies` unscoped". That is TS-era. rs has **no anomaly push sweep at all**: anomalies are pull-only (`http/anomalies.rs`). The PA now gets stale cards, the prime's included, inside its idle-edge digest (`pa_watchdog.rs:921` `stale_cards`), and pa.md L27-30 forbids timers and re-fetching. The two files contradict each other.
2. **WRONG**: protocol L123 says "Compaction is always the seat's own choice. Nobody forces it on another seat". pa.md L44 has the PA run `pij send <seat> --command compact`, and `core/control.rs:77` lets *any* actor send `compact` (it is exempt from the self/parent ownership check).
3. **WRONG (internal conflicts)**: orchestrator L96-97 says to poke an idle, silent worker with `COMPLETE/CONTINUING/BLOCKED`, while protocol L122 says "never message an idle seat to check on it". Also orient-oprime duty 4 says "Push tree-updates to all live streams on every roster change", while protocol rule 2 says broadcast only to seats whose next work it changes.
4. **STALE**: orient-oprime L102-104 says a PA "is refused … `spine-append` (pij#102)". In rs, `pij spine append` has no role gate (`daemon/http/governance.rs:1445`, and no `pa` role check anywhere in the daemon except the watchdog). Issue refs #89/#96/#102/#103/#446 do not resolve on canonical `AI-Substrate/pij-fleet` (`gh issue view` returns "Could not resolve").
5. **MISSING (pa.md)**: `--command compact` is refused for **Copilot** seats (`E-RS-CONTROL-UNSUPPORTED`, `control.rs:9,50`) and for paneless seats, and orchestrator's default coder and reviewer are Copilot. Controls also **bypass the cold-wake guard** (`send_control` never calls `cold_wake`), so the ❄ rule in pa.md is the only brake.
6. **MISSING**: `status-stale` fires for any non-parked seat whose card is more than 30 min old, including `done`, `ready` and idle seats (`anomalies.rs:270-300`, filtered only by waiting/hold/blocked/question). The "send it `pij report now`" remediation in pa.md, orient-oprime and orchestrator therefore wakes idle, possibly cold seats. That conflicts with the ❄ and "no blind messages" rules.
7. **STALE**: orchestrator L61 says "default-stack splits in your window", but rs spawn always creates a new window (`tmux/src/lib.rs:1125`, no `--layout`). Placement now means a manual `join-pane` per C5. Orchestrator L96 counts on "blocked/stalled push": rs pushes only obituaries (`death_sweep.rs`) and seat-self stall nudges. `report blocked`/`question` are not pushed to the parent; they are read with `pij decisions`/`pij state`.
8. **STALE/MISSING governance**: pending decisions live in "spine § Pending decisions" (orient-oprime duty 12, protocol L151), but rs has durable `pij report question` → `pij decisions` / `pij answer` (`daemon/http/decisions.rs`). Allocation via the "spine reservation ledger", the baton book and the hand-rolled canary have rs verbs (`pij stream create --ordinal`, `pij orchestration baton.*`, `pij canary --expect-model`) that these levers never name. `government-root.sh` **refuses in canonical pij-fleet** (there is no `prime-governance` branch; it exists only in `~/pi-hacking/pij-legacy`), so orchestrator entry step 2 cannot run here.
9. **STALE**: protocol L125 says "Platform guards that will enforce this are tracked in issue #446", but the guard shipped (`core/cold_wake.rs`, plan 157), and L117 of the same file already says so.
10. **BLOAT**: orient-oprime L53-110 is a 58-line epistemics callout inside a boot prompt, and orchestrator L102-108 restates orient-oprime duty 7 almost verbatim.

---

## orchestrator.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 34-35 | Read `<government-root>/orient-local.md` | `harness/scripts/government-root.sh` exits 1 in `~/pi-hacking/pij` ("expected one standing prime-governance worktree; found 0"). `git show-ref` shows no `prime-governance` ref in pij-fleet; it exists only in `pij-legacy` (refs/heads/prime-governance e05661c). The step is unrunnable in the canonical repo until the branch is restored. | UNVERIFIABLE (env gap; flag it) |
| 33-36 | Entry order: global → local orient → brief | orient-global L110 orders "protocol → local orient → item brief"; protocol.md is missing from this list | WRONG (inconsistent with orient-global) |
| 42 | Stop at `WAITING_FOR_BUILD_CONFIG` | The token appears only in pij skill files (orchestrator, protocol, stream-brief template), not in the Builder skill (`grep -r ~/.agents/skills/builder` finds 0). It is a pij label, not a Builder state. | UNVERIFIABLE (convention only; say so) |
| 45 | `/pij pair start … --coder-model --reviewer-model` | The route grammar exists (`routes/pair.md:203`), but the `flow-pair` skill is **not installed** in `~/.agents/skills` or `~/.claude/skills`; it exists only at `skills/flow-pair/lib/cli.ts`, and that `start` has no model flags. The `<flow-pair skill root>` the route requires does not resolve on this machine. | UNVERIFIABLE (precondition absent) |
| 61 | "Workers are default-stack splits in your window" | `pij-rs spawn --help` has no layout flag, and `tmux/src/lib.rs:1125` only runs `new-window`. "default-stack" is the retired TS `--layout stack` default. C5 (`00-routing.md:93`) now says to move the reported pane with `join-pane`. | STALE |
| 96 | wait for "completion, blocked, stalled, or dead push" | Only `dead` is pushed to a parent (`death_sweep.rs:81` obituaries). Stall nudges go to the seat itself (opt-in watchdog, `core/watchdog.rs:431`). `report blocked`/`question` open state/decision rows but deliver nothing to the parent (`http/decisions.rs` `question()` has no delivery). The parent learns by reading `pij decisions` / `pij state`. | STALE |
| 96-98 | Idle without report → send a request once | Conflicts with protocol L122 ("never message an idle seat to check on it… Stop after two quiet rounds") | WRONG (internal conflict) |
| 104 | "Relay its literal remediation: `pij report now …`" | The row's actual `remediation_line` is `pij send <seat> "Refresh your stale card with: pij report now '<what I just did>' '<what is next>'"` (`core/anomalies.rs:332`). Also, `done`/`ready`/idle seats flag after 30 min (`STATUS_STALE_MS`, `anomalies.rs:22,270-300`), so "close every row" can mean waking idle or cold seats. | STALE (wording) + MISSING (caveat) |
| 102-108 | Card-freshness paragraph | Near-verbatim duplicate of orient-oprime duty 7 L111-123 | BLOAT |
| 112-114 | `protocol.md` for "the ruled shared-tree fallback" | protocol only mentions it in passing (L83-84); the ruled rule is `rituals/kickoff.md § Shared-tree fallback yield rule` | STALE (pointer) |

Verified OK: `pij report now|question|blocked|state done` grammar and the state-word list (`pij-rs report --help`: `ready|waiting|hold|blocked|question|done|failed|cancelled`, no `working`); `pij anomalies` `--here`/`--project` semantics (`http/anomalies.rs:203-219`: `--here` = exact seat folder, `--project` requires an assignment so status-stale is always dropped); `gpt-5.6-sol` + `xhigh` valid (`harnesses/src/models.rs:14,243`); `/builder 8 ship`; links `orient-global.md`, `rituals/bootstrap.md#governance-branch--the-rules`, `../routes/pair.md`, `rituals/reports.md`, `rituals/batons.md`, `00-routing.md § C10/C11` all resolve; "Completion is `report state done`, never a watchdog self-pause".

**~22 claims checked OK.**

## orient-global.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 30-31 | "government files (spine, baton book) are the o-prime's" | Batons are rs store records (`pij orchestration baton.define/request/grant/return/reclaim`, `governance.rs:445-449`; `rituals/store-native.md:40`). The spine is `pij spine append/events/render`. Prose files exist only for unmigrated governments. | STALE |
| 37-39 | Canary = nonce round-trip + registry row + pane probe + second send | rs ships `pij canary <id> --expect-model <m> [--wait]` (nonce-correlated dispatch/ack plus model evidence, `governance.rs:440`; `00-routing.md:75`). The portable lever never names it. | MISSING |
| 61 (rule 9) | "Compact a worker before its next big packet" | Conflicts with protocol L123 ("Compaction is always the seat's own choice. Nobody forces it on another seat") | WRONG (internal conflict; see protocol) |
| 102 | "velocity curve (16h → 15m)" | Not checkable from code; a docs-only figure | UNVERIFIABLE |

Verified OK: one-hop escalation and context-owner-asks rules match protocol and AGENTS.md; no modal question UI; adopt → orient → preamble → work matches protocol L53; report contract fields match protocol L88-97 and `rituals/reports.md`; `rituals/batons.md` link resolves.

**~12 claims checked OK.**

## orient-oprime.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 20-22 | "unmigrated prose owns spine, baton book and prime-flow" | Stated as the general case. rs has store verbs for each: spine (`pij spine`), batons (`pij orchestration baton.*`), portfolio (`pij project create/set`, `pij stream create`). Prose ownership applies only to a not-yet-migrated government (`store-native.md:3-7`). | STALE (framing) |
| 24-25 | Portfolio states `proposed → deciding → … → dropped` | Prose only (prime-flow.json). The rs `Project` has no status field (`core/orchestration/governance.rs:9-24`), so the store cannot hold this lifecycle. | MISSING (say it is prose-only) |
| 28-30 | Allocate ordinal from "the spine's reservation ledger" | rs allocation is `pij stream create --project --slug [--base] [--ordinal] [--root]` (`governance.rs:433`; `rituals/kickoff.md:8,15`) | STALE |
| 36-37 | "Push tree-updates to all live streams on every roster change" | Conflicts with protocol L118 (broadcast only to seats whose next work it materially changes) and the ❄ guard. `pij tree` is also `E-RS-UNPORTED` (`pij --help`). | WRONG (internal conflict) + STALE |
| 53-110 | Lateral-verification callout | 58 lines in a boot prompt, against the token-lean skill rule. Its own "grep returns zero hits" claim is now self-falsified (the callout is the hit). It cites #89/#96/#103, which do not resolve on pij-fleet. | BLOAT |
| 102-104 | PA "refused … `spine-append` (pij#102)" | rs `spine append` has no role gate (`governance.rs:1059,1445`). The only `pa`-role checks in the daemon are in `http/watchdog.rs:184,214`. `spine-append` is also not an rs verb spelling (`pij spine append`). | STALE |
| 111-114 | `status-stale` is the sensor for "subordinates forget to report" | A seat that **never** filed a card never flags (`anomalies.rs` skips seats with no card), so the sensor misses the "never reported" case | MISSING |
| 117-118 | Relay `pij report now` or a parked state | `done`/`ready`/idle seats also flag after 30 min (`anomalies.rs:270-300`), so chasing every row wakes idle or cold seats, against protocol rules 1/6 | MISSING (caveat) |
| 139-142 | Prime's stale row "DROPPED by the anomaly sweep… `target === null`… PA closes it by POLLING `pij anomalies`" | TS-era (`target === null` is JS; the only "no effective parent" text left is in `.omp/extensions/pij/core/tree.ts`). rs has no anomaly delivery sweep. Status-stale reaches the PA through its digest (`pa_watchdog.rs:921,1007` `stale_cards` over every scope row, the prime included), and pa.md forbids polling. | WRONG |
| 155-156 | Rulings into "the spine's rulings log" | Store-native form is `pij spine append --kind ruling --refs path:<file>` (`store-native.md:14,50`) | STALE (prose-only framing) |
| 158-161 | Pending decision → "spine § Pending decisions" | rs durable decisions: `pij report question "<text>"` opens `decision-<hex>` (`http/decisions.rs:89-146`), read with `pij decisions [--state open\|answered\|all] [--asked_by] [--parent]`, answered with `pij answer <id> '<text>'` by the asker or the current parent (`core/decisions.rs:45`). Not mentioned. | MISSING |
| 6 (duty) | Verify, then relay | No mention of `pij report verify <seat> --assignment <id>` or the `unverified-done` anomaly (`anomalies.rs:390-418`), which is the store-native receipt for this duty | MISSING |
| 181 | `revive --attach` unported | Still true: `pij-rs revive --help` has `--assume-dead/--evidence/--fresh/--session/--name`, no `--attach` | OK (listed for the record) |

Verified OK: `pij whoami --json`, `pij phonehome --json`, `pij list --json`, `pij node show <id> --json` (`governance.rs:444,1054`); the list role/prime filters are refused (`pij --help`); `rituals/bootstrap.md § 5` (PA) exists; `rituals/store-native.md` and the bootstrap anchor resolve; `--project`/`--here` reasoning in duty 7 matches `http/anomalies.rs:203-219`; prime designation is a separate rs record (`orchestration prime.set`); `revive --attach` unported; C1 exists in `00-routing.md:28`.

**~18 claims checked OK.**

## protocol.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 37-43 | Government files / `<government-root>` | The resolver refuses in canonical pij-fleet (no `prime-governance` branch). The file set (`spine.md`, `baton-book.md`, `prime-flow.json`, `orient-local.md`) matches only `pij-legacy`'s branch (`git ls-tree prime-governance .harness/government/`). | UNVERIFIABLE (env gap) |
| 117 | ❄ guard: >400k and idle >1 h | Matches `core/cold_wake.rs:20,23,184-193` (Working → `busy`; unknown facts allow the send) | OK |
| 115 | RCA figures (38.6 h, 84%, 64%…) | The workspace exists (`~/games/unasphere/scratch/usage-blowout/`), but the figures were not recomputed | UNVERIFIABLE |
| 122 | "never message an idle seat to check on it" | Conflicts with orchestrator L97 and pa.md L51-52 ("send one specific ask"); one of them must yield | WRONG (internal conflict) |
| 123 | "Compaction is always the seat's own choice. Nobody forces it on another seat" | pa.md L44 `pij send <seat> --command compact`. `core/control.rs:77` exempts `compact` from the self/parent ownership check (any actor may compact any non-Copilot pane seat). | WRONG |
| 123 | Prime compacts itself at a >400k boundary | pa.md L70 says the prime compacts "on Jordan's word" | WRONG (internal conflict, minor) |
| 125 | "Platform guards … tracked in issue #446" | The guard shipped (`core/cold_wake.rs`, plan 157, `E-RS-COLD-WAKE`), and #446 does not resolve on pij-fleet | STALE |
| 151 | Pending decision → "spine § Pending decisions" | rs `pij report question` / `pij decisions` / `pij answer` (see orient-oprime) | MISSING |
| 136-137 | "fleet panes inside the stream's window" | Policy still matches C5, but rs spawn creates its own window; placement needs a manual `join-pane` | OK (policy); mechanism note in orchestrator |

Verified OK: `--fyi` semantics (held to the next turn; never meets the guard, `http/shim.rs:509`); `--force --reason` audited (`send.cold-wake-forced`); the `peer.md#converse` anchor resolves; templates (`spine.md`, `baton-book.md`, `orient-local.md`, `stream-brief.md`), `exemplars/canary-record.md`, `rituals/{kickoff,reports,batons}.md` exist; `/pij prime` route exists (`routes/prime.md`); `/builder 8 ship`; C10/C11 sections exist; the role table and one-hop escalation are consistent with AGENTS.md.

**~22 claims checked OK.**

## pa.md (against the PA watchdog shipped in bb9f0b1)

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 29-30 | Digest carries "context, idle time, cache warmth, ❄ price, turn and declared state" | The digest columns are `seat, role, state, declared, ctx, idle, cache[❄ $X to wake]` (`core/pa_digest.rs:154-187`). There is no "turn" field (`state` is the system state). | WRONG (nit) |
| 29-30 | Needs a look list | It holds nudgeable seats quiet ≥ interval, parked seats, and **stale cards only**. The digest itself says "`pij anomalies` has every kind" (`pa_digest.rs:137`). The brief never says other kinds (unverified-done, dead, open decisions) are absent, and L30 discourages re-fetching. | MISSING |
| 24-25 | Usage-limit stop → no idle nudge, one `usage limit lifted` wake | True, but only **Claude** seats' quota rejections are detected (`pa_watchdog.rs:532`, `usage_limit.rs` transcript `quotaLimits`). There is one wake per `(scope, resetsAt)`, so different resets can give more than one wake. | MISSING (scope caveat) |
| 18-23 | Prime-active wake | It requires a scope prime (designated, else lowest-id `prime`-role; none → never armed, `watchdog.rs:72-85`) **and** the PA itself observed `idle` with the prime at a tick after the edge. So "end your turn" is load-bearing for the morning wake. Neither is stated. | MISSING |
| 14-17 | One nudge per idle edge; your turns never trigger one | Correct, but a daemon restart sends nothing until a worker works again (`IdleEdge` first observation only arms, `watchdog.rs:53-66`). Also, the PA's own sends that make a worker work **will** produce the next nudge, which is what L45 "verify on the next nudge" depends on. A compaction that never shows the seat `working` may leave no next nudge. | MISSING |
| 44 | `pij send <seat> --command compact` | `control.rs:48-58` refuses Copilot targets (`E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls`) and paneless targets. Orchestrator's default workers are Copilot gpt-5.6-sol. | MISSING |
| 44, 62-65 | Cold-seat rules / "never `--force` past the cold-wake guard" | `--command` goes through `send_control` (`http/mod.rs:1905+`), which never consults `cold_wake`, so compacting a cold seat is **not** refused by the daemon. The rule here is the only brake. | MISSING |
| 55-56 | `status-stale`: send the seat its `pij report now` line | Stale flags include idle, `done` and `ready` seats (`anomalies.rs:270-300`), and the row's detail falsely says "has been working". Following this rule on an idle ❄ seat breaks the L62 rule. | MISSING (conflict caveat) |
| 70 | Prime "compacts itself on Jordan's word" | protocol L123 says the prime compacts itself at a >400k work boundary as policy | WRONG (internal conflict, minor) |

Verified OK: sender `pij-bg` (`core/lib.rs:42`); idle-nudge text `[pij watchdog] fleet idle …` (`pa_digest.rs:107`); prime-active text exact (`watchdog.rs:94`); supervisors (pa, prime role, designated prime) never count (`watchdog.rs:37-48`); scope = every live seat sharing the git common dir, all worktrees (`pa_watchdog.rs` `repo()`); the causality brake (pij delivery to the prime after arming → no wake, cap kept) and the residual "at most once per episode" match `watchdog.rs` `PrimeVerdict::Braked/Capped` and `docs/how/pa-watchdog.md:52-60`; prime-active suppressed on a limit (`OnLimit`); limit wake text "usage limit lifted" (`usage_limit.rs:181`); `pij compact-self` (`http/shim.rs:437-459`); the `--force --reason` cold guard exists; doc link `../../../../docs/how/pa-watchdog.md` resolves; no-timers rule matches the digest footer (`pa_digest.rs:189-191`); idle-flag interval = `PIJ_RS_WATCHDOG_SECS`, default 20 min (`config.rs:191`).

**~27 claims checked OK.**
