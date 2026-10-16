# Plan 175 findings ledger

One row per survey row (`assets/survey/{core,routes,prime-core,prime-rituals}.md`; 140 rows: 27 WRONG, 41 STALE, 50 MISSING, 7 BLOAT, 15 UNVERIFIABLE). Ids are `<report>-<nn>` in report order. **Where** is the file:line after the change, under `skills/pij/`. Each row was re-checked against `pij`/`pij-rs --help` or the code before acting; the evidence column names what was checked.

**Tally**: FIXED 115 · RULED 22 · REJECTED 0 · DEFERRED 0 · UNVERIFIABLE kept 3 (each says why). WRONG rows left DEFERRED: 0.

**Issues**: filed [pij-fleet#58](https://github.com/AI-Substrate/pij-fleet/issues/58) (status-stale flags idle/ready/done seats and says "has been working"). The skill side of pc-08, pc-22 and pc-41 is FIXED; the code side is that issue. The existing [#55](https://github.com/AI-Substrate/pij-fleet/issues/55) covers the revive double-seat gap (ro-18, R7).

**Found beyond the survey** (fixed, not counted above):
- SKILL.md:8 used `pij spawn` as a CLI example, but the shim refuses it.
- orchestrator.md gave the coder and the reviewer the same default model (gpt-5.6-sol), which contradicts the pair route's cross-model rule.
- My first ops/peer revive text said revive "checks nothing" for a tombstoned seat. revive.rs:147-197 shows the conversation-live check still runs, so the text was corrected.
- The shim-only diagnostic `pij generation` was not mentioned anywhere.
- peer.md's "any registered peer may compact" is now labelled as mechanism, with protocol rule 7 as the rule.

| id | report · file :line | verdict | claim (short) | disposition | where | evidence |
|---|---|---|---|---|---|---|
| co-01 | core · C. Verbs marked E-RS-UNPORTED that now work (natively or via a substitute) :`telegram` (L46) | STALE | refuses | FIXED | SKILL.md:70 | `pij-rs sidecar telegram --help`; docs/how/pij-telegram.md:10 |
| co-02 | core · C. Verbs marked E-RS-UNPORTED that now work (natively or via a substitute) :`daemon` (L46) | STALE | refuses | FIXED | SKILL.md:45, references/00-routing.md:165 | `pij-rs daemon --help` lists `bounce` |
| co-03 | core · C. Verbs marked E-RS-UNPORTED that now work (natively or via a substitute) :`chore` (L53) | UNVERIFIABLE | refuses | RULED R3 | SKILL.md:55 | chore stays a named refusal; `sidecar` row names only `telegram` |
| co-04 | core · C. Verbs marked E-RS-UNPORTED that now work (natively or via a substitute) :`tree` (L43) | MISSING | refuses | FIXED | SKILL.md:55 | `pij-rs node --help` "Read a seat and its governed work tree" |
| co-05 | core · SKILL.md :3 | STALE | description: "run pij agent packs (flowspace-search etc.)", "run flow-pair … fleets" | FIXED | SKILL.md:3 | agent packs dropped from triggers (`pij agent` refuses); pair = builder seats (R1) |
| co-06 | core · SKILL.md :24 | STALE | `pair` = "wrapping the-flow" | RULED R1 | SKILL.md:24 | registry row names /builder 6 implement + 7 review |
| co-07 | core · SKILL.md :26, 86 | MISSING | `agent` registry row and alias "run flowspace search / ask an agent" → `/pij agent`, with no caveat | FIXED | SKILL.md:26, SKILL.md:97 | `pij --help`: `pij agent: E-RS-UNPORTED` |
| co-08 | core · SKILL.md :41 | MISSING | send row lists `--fyi` only | FIXED | SKILL.md:41 | `pij send --help` `--force --reason` |
| co-09 | core · SKILL.md :43 | MISSING | `tree` unported | FIXED | SKILL.md:55 | same as co-04 |
| co-10 | core · SKILL.md :46 | STALE | `telegram` → "shim E-RS-UNPORTED; use documented native daemon lifecycle" | FIXED | SKILL.md:45 | same as co-01 |
| co-11 | core · SKILL.md :46 | MISSING | `daemon` → "documented native daemon lifecycle" | FIXED | SKILL.md:45, references/00-routing.md:165 | `pij-rs daemon bounce --help` |
| co-12 | core · SKILL.md :51 | MISSING | `bg` (`create/list/tail/kill`) | FIXED | SKILL.md:49 | RS_ROUTE_TABLE row `bg`/`emit`; TS reverse gate now enforces leaves |
| co-13 | core · SKILL.md :53 | UNVERIFIABLE | `chore` "named E-RS-UNPORTED; no legacy escape" | RULED R3 | SKILL.md:55 | as co-03 |
| co-14 | core · SKILL.md :36-57 | MISSING | the table is the "every `pij` verb has a home" inventory | FIXED | SKILL.md:62, SKILL.md:73 | crates/cli/tests/skill_verb_coverage.rs + TS reverse check; red receipts assets/receipts/tk-0001-* |
| co-15 | core · SKILL.md :45,46,48,53 | BLOAT | four rows of refusal-only verbs | FIXED | SKILL.md:55 | 4 refusal-only rows collapsed to one |
| co-16 | core · SKILL.md :62 | BLOAT | restates generation=legacy, raw `--json` envelope, fixtures path | FIXED | references/00-routing.md:5 | one copy kept in SKILL.md (always loaded); 00-routing cites it |
| co-17 | core · SKILL.md :66 | STALE | invariant 1: never write `.the-flow-state.json` / `the-flow.json` / `the-flow.md` | RULED R6 | SKILL.md:77 | `.the-flow-state.json` dropped; builder SKILL.md: "There is no .the-flow-state.json" |
| co-18 | core · SKILL.md :77 | WRONG | "the daemon's watchdog serves PAs only" | FIXED | SKILL.md:88 | `pij-rs watchdog --help`: any seat opts in |
| co-19 | core · SKILL.md :77 | UNVERIFIABLE | "2026-07-31 ruling at the resolved government root" | FIXED | SKILL.md:88 | government root unresolvable on pij-fleet (R2); now cites the ruling where it is recorded |
| co-20 | core · SKILL.md :84 | STALE | `/flow-pair start\ | RULED R1 | SKILL.md:95 | alias kept; resolves to the rewritten pair route |
| co-21 | core · SKILL.md :92 | STALE | References: "Shared conventions (C1 … C7)" | FIXED | SKILL.md:103 | 00-routing defines C1-C12 |
| co-22 | core · references/00-routing.md :5 | BLOAT | Same content as SKILL.md L42/L62 (legacy forcing, raw `--json`, list/sessions filters, fixtures) | FIXED | references/00-routing.md:5 | duplicate paragraph removed |
| co-23 | core · references/00-routing.md :14 | UNVERIFIABLE | Signal A: newest `.flow-pair/runs/*/run.json` with `status: open` | RULED R1 | references/00-routing.md:13 | flow-pair ledger retired; signal now reads the rs roster |
| co-24 | core · references/00-routing.md :16 | STALE | Signal C: newest `docs/plans/*/the-flow.json` → `harness flow nav show --path` | FIXED | references/00-routing.md:14 | plans use plan.dd.json (docs/plans/173, 175) |
| co-25 | core · references/00-routing.md :36-43 | MISSING | C1 delivery table: rows for prereq/spawn/message/FYI/receive/compact/peek/retire | FIXED | references/00-routing.md:39, references/00-routing.md:71 | `pij send --help`; .omp/extensions/pij/index.ts:91 `force` |
| co-26 | core · references/00-routing.md :37 | MISSING | tmux spawn = `pij-rs spawn --harness <h> --cwd <abs> --parent <id> …` | FIXED | references/00-routing.md:36 | `pij-rs spawn --help` `--role`, `--name` |
| co-27 | core · references/00-routing.md :38 | MISSING | message = `pij send <id> '<text>'` | FIXED | references/00-routing.md:37 | `pij-rs send --help`; docs/how/pij-federation.md |
| co-28 | core · references/00-routing.md :45 | WRONG | "Existing-seat structural repair through `link` is unported, so never omit parent assuming that command can… | FIXED | references/00-routing.md:45 | `pij --help`: `pij link: POST /v1/link`; role.rs link parse |
| co-29 | core · references/00-routing.md :93 vs 102-110 | STALE | "This applies to every layout (`window` names the window; `stack`/`right`/`below` name the pane title)" | FIXED | references/00-routing.md:111 | `pij-rs spawn --help` has no `--layout` |
| co-30 | core · references/00-routing.md :145 | WRONG | "spawn with --layout window, then move" | FIXED | references/00-routing.md:147 | same |
| co-31 | core · references/00-routing.md :95-112 | MISSING | Naming mandate: rename the window after spawn | FIXED | references/00-routing.md:101 | `pij-rs spawn --help` `--name` (defaults to seat id) |
| co-32 | core · references/00-routing.md :163 | MISSING | C6: "Check the installed `pij-rs daemon --help`" for lifecycle | FIXED | references/00-routing.md:165 | `pij-rs daemon bounce --help`; justfile bounce-rs |
| co-33 | core · references/00-routing.md :202 | STALE | "`pij send <self>` is still E-SELF" | FIXED | references/00-routing.md:204 | crates/core/src/error.rs:140 |
| co-34 | core · references/00-routing.md :206 | STALE | C8: "persisted pij-owned close is **requested**; observed absence … **unrequested-by-pij**; … boot reconcil… | FIXED | references/00-routing.md:206 | crates/daemon/src/death_sweep.rs; no `unrequested`/`no-show` in crates/ |
| co-35 | core · references/00-routing.md :208 | STALE | "Each launch has a bounded expectation keyed by `spawnId` … descriptor with same key suppresses the no-show" | FIXED | references/00-routing.md:208 | crates/daemon/src/http/mod.rs:1477 "timed out after Ns waiting for registration" |
| co-36 | core · references/00-routing.md :204-208 | MISSING | C8 (what C8 should say) | FIXED | references/00-routing.md:208 | death_sweep.rs:18 (5000 ms), body() obituary, withheld for dead parents |
| co-37 | core · references/00-routing.md :212 | MISSING | C9: PA nudged at the fleet's idle edge and once at a usage-limit reset | FIXED | references/00-routing.md:212 | crates/core/src/watchdog.rs PrimeActive; docs/how/pa-watchdog.md § Prime-active wake |
| co-38 | core · references/00-routing.md :216 | UNVERIFIABLE | C10: "Evidence: plan 083 (~2.9M tokens …)" | UNVERIFIABLE (kept) | references/00-routing.md:216 | re-survey fix round 1: the evidence (~/pi-hacking/pij-legacy/docs/plans/083-a2a-wire-discipline) is outside this checkout, so it cannot be verified here; the figure is now attributed as operator-reported |
| co-39 | core · references/00-routing.md :248 | UNVERIFIABLE | C12: the "2026-09-29" load ~230 incident | UNVERIFIABLE (kept) | references/00-routing.md:248 | operator anecdote with no repo artifact, now attributed ("By operator report"); it motivates the caps but no claim depends on it |
| ro-01 | routes · peer.md :42-46 | MISSING | Revive grammar is `<seat> [--session] [--name]` plus `--assume-dead --evidence`; "--print/--attach unsuppor… | FIXED | references/routes/peer.md:56, references/routes/peer.md:42 | `pij-rs revive --help` `--fresh`; revive.rs:108-145 (E-RS-ARG, E-RS-REVIVE-CONVERSATION-LIVE) |
| ro-02 | routes · peer.md :54 | MISSING | "An `Active` process or a present recorded pane still vetoes revival" | FIXED | references/routes/peer.md:58 | revive.rs:294 `pane_present != Some(false)`; :245 tombstoned path has no veto |
| ro-03 | routes · peer.md :54/147 | MISSING | The reboot row says only "tombstone or Dead+pane absent" | FIXED | references/routes/peer.md:151 | points to the single reboot procedure |
| ro-04 | routes · peer.md :77 | BLOAT | The FYI rule, word for word | FIXED | references/routes/peer.md:81 | rule restatement cut; C10 holds it; docs/how link retargeted to C10 |
| ro-05 | routes · peer.md :77 | MISSING | The FYI mechanics | FIXED | references/routes/peer.md:81 | crates/core/src/fyi.rs:27 FLUSH_AT=5, DIGEST_ABOVE=5; `pij-rs fyi-read --help` |
| ro-06 | routes · peer.md :92 | MISSING | "`pij bg create/list/tail/kill`" | FIXED | references/routes/peer.md:96 | `pij-rs bg --help` |
| ro-07 | routes · pair.md :13 | WRONG | "Shim spawn/agent/tree/link/models are named refusals" | RULED R1 | references/routes/pair.md:1 | pair.md rewritten; no link claim remains |
| ro-08 | routes · pair.md :154 | WRONG | "Existing-seat reparent/root placement (`link`) is unported" | RULED R1 | references/routes/pair.md:1 | same; link documented once in peer route |
| ro-09 | routes · pair.md :6-11, 25-27 | STALE | Wraps `the-flow`; "`the-flow` guided mode is the sole writer of `.the-flow-state.json`, the-flow.json, the-… | RULED R1 | references/routes/pair.md:18 | the-flow wrapper removed; R6 file rule |
| ro-10 | routes · pair.md :13-21, 250 | STALE | `<flow-pair skill root>` = "the INSTALLED flow-pair skill … a sibling of this pij skill in the skills insta… | RULED R1 | references/routes/pair.md:9 | no `<flow-pair skill root>` dependency left |
| ro-11 | routes · pair.md :132-134 | WRONG | The roster lives in the ledger `run.json` as `role → {pijId,paneId,model,spawnedByUs}`, persisted by P9 | RULED R1 | references/routes/pair.md:25 | run.json roster removed |
| ro-12 | routes · pair.md :136, 203 | WRONG | `--coder-model` / `--reviewer-model` on `pair start "<request>"` | RULED R1 | references/routes/pair.md:76 | `pair start` flags removed |
| ro-13 | routes · pair.md :141-142 | STALE | Default models `github-copilot/claude-sonnet-4.6:xhigh`, `github-copilot/gpt-5.5:xhigh` | FIXED | references/routes/pair.md:84 | `pij-rs spawn --help` `--effort`; no stale default models |
| ro-14 | routes · pair.md :160 | UNVERIFIABLE | "`pij state` shows it stuck `pending`/`failed`" | FIXED | references/routes/pair.md:91 | StateCard `liveness` active/dead/recycled (`pij state --json`) |
| ro-15 | routes · pair.md :182 | STALE | "on the colleague's done-report (or a `stalled`/`dead` push)" | FIXED | references/routes/pair.md:115 | grep `stalled` in crates/daemon: none; death_sweep.rs obituary |
| ro-16 | routes · ops.md :58 | WRONG | Revive "is **not** the former `--print`, `--attach` or `--assume-dead` workflow, and requires a known tombs… | FIXED | references/routes/ops.md:52 | revive.rs:207-219, 294-399; `pij-rs revive --help` |
| ro-17 | routes · ops.md :50-58 | MISSING | The reboot procedure: re-observe, ping, then `pij-rs revive <tombstoned-seat>` | FIXED | references/routes/ops.md:56, references/routes/ops.md:58 | death_sweep.rs; reaper.rs:82 reincarnated; revive.rs:419 no_wait |
| ro-18 | routes · ops.md :58 | MISSING | Double-seat risk (#55) | FIXED | references/routes/ops.md:57 | R7: manual check kept; #55 OPEN (gh issue view 55); revive.rs:147-197 |
| ro-19 | routes · ops.md :58 | STALE | "In-place reattachment … remain[s a] named gap" | FIXED | references/routes/ops.md:57 | `pij-rs adopt --help` `--reclaim`; api § Adopt continuity order |
| ro-20 | routes · ops.md :46 | MISSING | The reap eligibility rule | FIXED | references/routes/ops.md:48 | crates/daemon/src/reaper.rs:17,82 |
| ro-21 | routes · ops.md :9-14 | MISSING | Daemon lifecycle: `pij-rs ping`, `pij-rs daemon --bind` | FIXED | references/routes/ops.md:13, references/routes/ops.md:16 | `pij-rs daemon --help`, `pij-rs peers --help`; `--offline`/`--insecure-bind` left to --help (not agent work) |
| ro-22 | routes · ops.md :75-77 | STALE | Government example `$HOME/pi-hacking/pij-worktrees/pij-governance` | RULED R2 | references/routes/ops.md:80 | `harness/scripts/government-root.sh` exit 1 "found 0" |
| ro-23 | routes · delegate.md :23-24 | STALE | The forbidden list includes `.the-flow-state.json`, the-flow.json, the-flow.md | RULED R6 | references/routes/delegate.md:23 | `.the-flow-state.json` dropped |
| ro-24 | routes · delegate.md :36 | MISSING | `pij-rs spawn … --parent <your-seat>` | FIXED | references/routes/delegate.md:36 | `pij-rs spawn --help` `--role` |
| ro-25 | routes · ready.md :22 | WRONG | "`pij adopt` rejects `--harness pi` by design" | FIXED | references/routes/ready.md:22 | identity_tests.rs:1310 adopts `pi` with 200 |
| ro-26 | routes · ready.md :34 | STALE | "native revive requires an explicit tombstoned seat" | FIXED | references/routes/ready.md:34 | revive.rs:301 accepts Dead+pane-absent; ready route defers to peer/ops |
| ro-27 | routes · ready.md :20-26 | MISSING | Claude seats run `pij adopt` | FIXED | references/routes/ready.md:24 | docs/how/pij-rs-api.md § Adopt continuity order (status line self-heal) |
| ro-28 | routes · node.md :12 | WRONG | `pij role <seat> reviewer --json` | FIXED | references/routes/node.md:12 | crates/core/src/orchestration/governance.rs:161 SEAT_ROLES |
| ro-29 | routes · node.md :101 | WRONG | "`link`/`tree` are unported … reparenting/root placement is a named gap" | FIXED | references/routes/node.md:101 | `pij --help` link served, tree refused |
| ro-30 | routes · node.md :65 | WRONG | "Use `--note` with question/blocked" | FIXED | references/routes/node.md:65 | crates/daemon/src/http/report.rs:204, 277 (question/blocked accept assignment, refs) |
| ro-31 | routes · node.md :98 | MISSING | `pij anomalies [--here] [--project]` | FIXED | references/routes/node.md:98 | crates/daemon/src/http/anomalies.rs:245 |
| ro-32 | routes · prime.md :59 | STALE | "broadcast only to … seats it changes; no acks or FYIs" | FIXED | references/routes/prime.md:59 | matches protocol rule 3 |
| ro-33 | routes · skill.md :13 | MISSING | spawn example | FIXED | references/routes/skill.md:13 | `pij-rs spawn --help` |
| pc-01 | prime-core · orchestrator.md :34-35 | UNVERIFIABLE | Read `<government-root>/orient-local.md` | RULED R2 | references/prime/orchestrator.md:35 | government-root.sh exit 1 |
| pc-02 | prime-core · orchestrator.md :33-36 | WRONG | Entry order: global → local orient → brief | FIXED | references/prime/orchestrator.md:33 | orient-global lifecycle: protocol → local orient → brief |
| pc-03 | prime-core · orchestrator.md :42 | UNVERIFIABLE | Stop at `WAITING_FOR_BUILD_CONFIG` | FIXED | references/prime/orchestrator.md:43 | token exists nowhere in builder skill or harness CLI; replaced by plain rule |
| pc-04 | prime-core · orchestrator.md :45 | UNVERIFIABLE | `/pij pair start … --coder-model --reviewer-model` | RULED R1 | references/prime/orchestrator.md:46 | pair start flags removed |
| pc-05 | prime-core · orchestrator.md :61 | STALE | "Workers are default-stack splits in your window" | FIXED | references/prime/orchestrator.md:64 | tmux/src/lib.rs new-window; C5 |
| pc-06 | prime-core · orchestrator.md :96 | STALE | wait for "completion, blocked, stalled, or dead push" | FIXED | references/prime/orchestrator.md:98 | death_sweep.rs; http/decisions.rs question() has no delivery |
| pc-07 | prime-core · orchestrator.md :96-98 | WRONG | Idle without report → send a request once | RULED R4 | references/prime/orchestrator.md:100 | 'poke once' removed |
| pc-08 | prime-core · orchestrator.md :104 | STALE | "Relay its literal remediation: `pij report now …`" | FIXED | references/prime/orchestrator.md:104 | remediation_line anomalies.rs:332; caveat + issue #58 |
| pc-09 | prime-core · orchestrator.md :102-108 | BLOAT | Card-freshness paragraph | FIXED | references/prime/orchestrator.md:103 | paragraph replaced by citation |
| pc-10 | prime-core · orchestrator.md :112-114 | STALE | `protocol.md` for "the ruled shared-tree fallback" | FIXED | references/prime/orchestrator.md:110 | anchor resolves (link check) |
| pc-11 | prime-core · orient-global.md :30-31 | STALE | "government files (spine, baton book) are the o-prime's" | FIXED | references/prime/orient-global.md:31 | store verbs per store-native.md |
| pc-12 | prime-core · orient-global.md :37-39 | MISSING | Canary = nonce round-trip + registry row + pane probe + second send | FIXED | references/prime/orient-global.md:37 | governance.rs:1793 canary_command |
| pc-13 | prime-core · orient-global.md :61 (rule 9) | WRONG | "Compact a worker before its next big packet" | RULED R5 | references/prime/orient-global.md:62 | parent's compaction is an allowed case |
| pc-14 | prime-core · orient-global.md :102 | UNVERIFIABLE | "velocity curve (16h → 15m)" | FIXED | references/prime/orient-global.md:103 | uncheckable figure removed |
| pc-15 | prime-core · orient-oprime.md :20-22 | STALE | "unmigrated prose owns spine, baton book and prime-flow" | FIXED | references/prime/orient-oprime.md:21 | store-native.md |
| pc-16 | prime-core · orient-oprime.md :24-25 | MISSING | Portfolio states `proposed → deciding → … → dropped` | FIXED | references/prime/orient-oprime.md:26 | crates/core/src/orchestration/governance.rs Project struct |
| pc-17 | prime-core · orient-oprime.md :28-30 | STALE | Allocate ordinal from "the spine's reservation ledger" | FIXED | references/prime/orient-oprime.md:30 | governance.rs:433 |
| pc-18 | prime-core · orient-oprime.md :36-37 | WRONG | "Push tree-updates to all live streams on every roster change" | FIXED | references/prime/orient-oprime.md:38 | protocol rule 2; `pij tree` refuses |
| pc-19 | prime-core · orient-oprime.md :53-110 | BLOAT | Lateral-verification callout | FIXED | references/prime/orient-oprime.md:57 | 58-line callout cut to 9; dead issue refs dropped |
| pc-20 | prime-core · orient-oprime.md :102-104 | STALE | PA "refused … `spine-append` (pij#102)" | FIXED | references/prime/orient-oprime.md:62 | spine append has no role gate (governance.rs append parse; only watchdog checks `pa`); claim removed |
| pc-21 | prime-core · orient-oprime.md :111-114 | MISSING | `status-stale` is the sensor for "subordinates forget to report" | FIXED | references/prime/orient-oprime.md:68 | anomalies.rs StatusStaleDetector `else { continue }` |
| pc-22 | prime-core · orient-oprime.md :117-118 | MISSING | Relay `pij report now` or a parked state | FIXED | references/prime/orient-oprime.md:73 | anomalies.rs:270-300; code side filed as #58 |
| pc-23 | prime-core · orient-oprime.md :139-142 | WRONG | Prime's stale row "DROPPED by the anomaly sweep… `target === null`… PA closes it by POLLING `pij anomalies`" | FIXED | references/prime/orient-oprime.md:94 | http/anomalies.rs read-only; pa_watchdog.rs:921 stale_cards in digest |
| pc-24 | prime-core · orient-oprime.md :155-156 | STALE | Rulings into "the spine's rulings log" | FIXED | references/prime/orient-oprime.md:109 | store-native.md mapping |
| pc-25 | prime-core · orient-oprime.md :158-161 | MISSING | Pending decision → "spine § Pending decisions" | FIXED | references/prime/orient-oprime.md:115 | http/decisions.rs |
| pc-26 | prime-core · orient-oprime.md :6 (duty) | MISSING | Verify, then relay | FIXED | references/prime/orient-oprime.md:49 | decisions.rs:298-331; anomalies unverified-done |
| pc-27 | prime-core · protocol.md :37-43 | UNVERIFIABLE | Government files / `<government-root>` | RULED R2 | references/prime/protocol.md:40 | absent case documented |
| pc-28 | prime-core · protocol.md :115 | UNVERIFIABLE | RCA figures (38.6 h, 84%, 64%…) | UNVERIFIABLE (kept) | references/prime/protocol.md:116 | source workspace is outside the repo (~/games/unasphere/scratch/usage-blowout/); figures are attributed, not recomputed; no rule depends on the exact numbers |
| pc-29 | prime-core · protocol.md :122 | WRONG | "never message an idle seat to check on it" | RULED R4 | references/prime/protocol.md:123 | protocol wins; orchestrator and pa aligned |
| pc-30 | prime-core · protocol.md :123 | WRONG | "Compaction is always the seat's own choice. Nobody forces it on another seat" | RULED R5 | references/prime/protocol.md:124 | control.rs:77 lets any actor compact: stated as mechanism vs rule |
| pc-31 | prime-core · protocol.md :123 | WRONG | Prime compacts itself at a >400k boundary | FIXED | references/prime/pa.md:77 | pa.md now cites protocol rule 7 |
| pc-32 | prime-core · protocol.md :125 | STALE | "Platform guards … tracked in issue #446" | FIXED | references/prime/protocol.md:126 | #446 does not resolve on pij-fleet; guard shipped |
| pc-33 | prime-core · protocol.md :151 | MISSING | Pending decision → "spine § Pending decisions" | FIXED | references/prime/protocol.md:152 | http/decisions.rs |
| pc-34 | prime-core · pa.md :29-30 | WRONG | Digest carries "context, idle time, cache warmth, ❄ price, turn and declared state" | FIXED | references/prime/pa.md:32 | crates/core/src/pa_digest.rs:154-187 columns |
| pc-35 | prime-core · pa.md :29-30 | MISSING | Needs a look list | FIXED | references/prime/pa.md:33 | pa_digest.rs:137 "anomalies here are stale cards only" |
| pc-36 | prime-core · pa.md :24-25 | MISSING | Usage-limit stop → no idle nudge, one `usage limit lifted` wake | FIXED | references/prime/pa.md:27 | usage_limit.rs quotaLimits; one wake per (scope, resetsAt) |
| pc-37 | prime-core · pa.md :18-23 | MISSING | Prime-active wake | FIXED | references/prime/pa.md:21 | docs/how/pa-watchdog.md § Prime-active wake; watchdog.rs scope_prime |
| pc-38 | prime-core · pa.md :14-17 | MISSING | One nudge per idle edge; your turns never trigger one | FIXED | references/prime/pa.md:18 | pa-watchdog.md § Idle nudge (Restart) |
| pc-39 | prime-core · pa.md :44 | MISSING | `pij send <seat> --command compact` | FIXED | references/prime/pa.md:49 | crates/core/src/control.rs:48-58 |
| pc-40 | prime-core · pa.md :44, 62-65 | MISSING | Cold-seat rules / "never `--force` past the cold-wake guard" | FIXED | references/prime/pa.md:69 | api § Cold-wake guard "Never guarded: ... controls" |
| pc-41 | prime-core · pa.md :55-56 | MISSING | `status-stale`: send the seat its `pij report now` line | FIXED | references/prime/pa.md:60 | skill side fixed (R4); code side DEFERRED to #58 |
| pc-42 | prime-core · pa.md :70 | WRONG | Prime "compacts itself on Jordan's word" | FIXED | references/prime/pa.md:77 | as pc-31 |
| pr-01 | prime-rituals · rituals/bootstrap.md :10 | STALE | "OMP and Pi each register through their runtime extension at boot" | FIXED | references/prime/rituals/bootstrap.md:10 | 00-routing C1 |
| pr-02 | prime-rituals · rituals/bootstrap.md :36 | STALE | `docs/plans/<ord>-<slug>/` stays on main "(code PR + the-flow archival)" | FIXED | references/prime/rituals/bootstrap.md:36 | builder SKILL.md registry 7b |
| pr-03 | prime-rituals · rituals/bootstrap.md :43 | UNVERIFIABLE | `harness flow create prime-flow … --path <government-root>/prime-flow.json` | FIXED | references/prime/rituals/bootstrap.md:43 | `harness flow create --help`: `--path` "must be in-repo"; not run (mutates) |
| pr-04 | prime-rituals · rituals/bootstrap.md :46 | STALE | "`AI-Substrate/pij` uses `$HOME/pi-hacking/pij-worktrees/pij-governance`; resolver `harness/scripts/governm… | RULED R2 | references/prime/rituals/bootstrap.md:46 | government-root.sh exit 1; dead example removed |
| pr-05 | prime-rituals · rituals/bootstrap.md :61 | STALE | PA clock = one idle-edge nudge plus one usage-limit reset wake | FIXED | references/prime/rituals/bootstrap.md:63 | pa-watchdog.md § Prime-active wake |
| pr-06 | prime-rituals · rituals/bootstrap.md :78 | MISSING | "Stream dies → explicit native revive/spawn" | FIXED | references/prime/rituals/bootstrap.md:80 | `pij --help` revive refuses; api § Revive |
| pr-07 | prime-rituals · rituals/kickoff.md :10-11 | MISSING | "Derive descriptive fences from actions" (no verb) | FIXED | references/prime/rituals/kickoff.md:10 | governance.rs:436-437 |
| pr-08 | prime-rituals · rituals/kickoff.md :18 | MISSING | spawn line has `--parent` but no role | FIXED | references/prime/rituals/kickoff.md:18 | `pij-rs spawn --help` |
| pr-09 | prime-rituals · rituals/kickoff.md :22 | WRONG | "tree/link remain unported" | FIXED | references/prime/rituals/kickoff.md:22 | `pij --help` |
| pr-10 | prime-rituals · rituals/kickoff.md :26 | STALE | "existing-seat reparenting is a named gap, not a role side effect" | FIXED | references/prime/rituals/kickoff.md:25 | role.rs link: parent absent/not live only |
| pr-11 | prime-rituals · rituals/kickoff.md :23-25 | STALE | `pij canary <id> --expect-model <m>`, then "complete leg (a) round-trip and leg (b) identity proof" by hand | FIXED | references/prime/rituals/kickoff.md:23 | governance.rs:1793-1955 |
| pr-12 | prime-rituals · rituals/kickoff.md :51-55 | STALE | Canary table: (a) "send a nonce challenge; ack arrives as injected turn"; (b) "capture pane footer as fallb… | FIXED | references/prime/rituals/kickoff.md:56 | E-RS-CANARY-PENDING / E-RS-CANARY; default wait 5000, max 300000 (governance.rs:503-508) |
| pr-13 | prime-rituals · rituals/kickoff.md :27-29 | MISSING | `pij dispatch <id> --packet <brief> --wait` | FIXED | references/prime/rituals/kickoff.md:30 | governance.rs:1678 wait_dispatch(…, false) |
| pr-14 | prime-rituals · rituals/kickoff.md :35 | MISSING | Retirement: "retire allocation/history without erasure" | FIXED | references/prime/rituals/kickoff.md:37 | governance.rs:435, 443 |
| pr-15 | prime-rituals · rituals/kickoff.md :8 | MISSING | `pij stream create --project <p> --slug <s> [--base <ref>] [--ordinal N]` | FIXED | references/prime/rituals/kickoff.md:8 | governance.rs:433 |
| pr-16 | prime-rituals · rituals/batons.md :33-35 | MISSING | grant "stale/unverifiable requires explicit `--repin`" | FIXED | references/prime/rituals/batons.md:34 | governance.rs:1172 |
| pr-17 | prime-rituals · rituals/batons.md :33, 41 | MISSING | Keeper grants; reclaim is explicit | FIXED | references/prime/rituals/batons.md:36 | governance.rs:1212 only return is holder-gated |
| pr-18 | prime-rituals · rituals/batons.md :75-89 | BLOAT | "Cause and expiry" section (koala, araminta, pij#72, stale badges, PA exemptions) | FIXED | references/prime/rituals/batons.md:76 | 15-line section cut to 3; dead pij#72 ref dropped |
| pr-19 | prime-rituals · rituals/reports.md :1-20 | MISSING | Reports are evidence files plus a pointer send | FIXED | references/prime/rituals/reports.md:27, references/prime/rituals/reports.md:4 | decisions.rs:298-331 |
| pr-20 | prime-rituals · rituals/incidents.md :38 | STALE | broadcast "(tree-push channel)" | FIXED | references/prime/rituals/incidents.md:40 | `pij --help` |
| pr-21 | prime-rituals · rituals/store-native.md :16-20 | WRONG | `--project` on `task set` → "every later state-set/state-verified event inherits it", else orphaned from th… | FIXED | references/prime/rituals/store-native.md:21 | governance.rs:1492-1504 project filter; report.rs payloads |
| pr-22 | prime-rituals · rituals/store-native.md :16-20 | WRONG | Paragraph placed between table rows | FIXED | references/prime/rituals/store-native.md:16 | paragraph moved out of the table; rows render |
| pr-23 | prime-rituals · rituals/store-native.md :22 | WRONG | "tree/link are unported" | FIXED | references/prime/rituals/store-native.md:18 | `pij --help` |
| pr-24 | prime-rituals · templates/stream-brief.md :54 | STALE | "A validated plan stops at `WAITING_FOR_BUILD_CONFIG`" | FIXED | references/prime/templates/stream-brief.md:54 | as pc-03 |
| pr-25 | prime-rituals · templates/stream-brief.md :65 | WRONG | identity "role `stream-s<ORD>`" | FIXED | references/prime/templates/stream-brief.md:65 | SEAT_ROLES |
| pr-26 | prime-rituals · exemplars/canary-record.md, exemplars/grant-log.md :canary-record:24, grant-log:28,33 | UNVERIFIABLE | "Source: vendored war story N" | FIXED | references/prime/exemplars/canary-record.md:24 | dangling 'vendored war story' citations removed (no source in pij-fleet or pij-legacy) |
