# Skill survey — partition `core`

Files: `skills/pij/SKILL.md` (93 lines), `skills/pij/references/00-routing.md` (248 lines). Repo main @ bb9f0b1. Raw help dumps: `docs/plans/175-pij-skill-realign/assets/survey/core-help/rs-*.txt`.

## Summary (severity-ranked)

1. **WRONG** 00-routing L45: "structural repair through `link` is unported". `link` shipped in plan 166, and SKILL.md L44 says so. Both files are in the core partition, so they contradict each other.
2. **WRONG** 00-routing L145: "spawn with `--layout window`, then move". Native spawn has no `--layout`, and the same section (L93, L153) says not to pass it.
3. **WRONG** SKILL.md L77 (invariant 12): "the daemon's watchdog serves PAs only". This contradicts L52/C9 and the code: any seat may opt in with `pij watchdog on`.
4. **STALE** 00-routing C8 (L206-208) uses legacy-TS vocabulary: "requested / unrequested-by-pij", "historical" boot reconciliation, the spawnId no-show expectation. None of these exist in crates/. The rs replacement is missing: a 5s death sweep that pushes an obituary from `pij-bg` to the recorded parent.
5. **STALE** Guided signal C (00-routing L16) can never fire. No plan has a `the-flow.json`; plans now use builder `plan.dd.json`. `pair` "wrapping the-flow" (SKILL L24) and invariant 1's the-flow file list are stale for the same reason.
6. **MISSING** Shipped surfaces that appear nowhere in the skill:
   - the plan 173 prime-active PA wake (merged at HEAD);
   - federation (`--to <seat>@<machine>`, `pij-rs peers`);
   - native Telegram outbound (`pij-rs sidecar telegram`);
   - `pij-rs daemon bounce`;
   - a forced-cold-wake row in the C1 delivery table (`--force --reason` / `force:true`).
7. **STALE**
   - SKILL.md References lists C1–C7, but C1–C12 exist.
   - C7 cites `E-SELF`, which is a legacy TS code; rs refuses self-sends with kind `refused`.
   - The `agent` route and alias are advertised in the frontmatter with no "unported" caveat.
8. **Structural (encode, don't document)**:
   - The verb-coverage gate (`generation-skill-coverage.test.ts`) only checks skill ⊂ shim table. Drift from native `pij-rs` verbs is invisible to it, and it forbids adding a native-only verb as a top-level row.
   - Neither the test nor `pij-skill-check.sh` checks the shim→skill or pij-rs→skill direction.

---

## Verb inventory diff (CLI-verb coverage table vs live CLI)

Sources: `pij --help` (shim: `.omp/extensions/pij/core/generation-routing.ts` `RS_ROUTE_TABLE` L159-342), `pij-rs --help` plus every `pij-rs <verb> --help`, and the daemon leaf grammar in `crates/daemon/src/http/governance.rs:429-455`.

### A. Verbs/leaves live but missing from the table

| verb / leaf | surface | what it is | evidence |
|---|---|---|---|
| `generation` | shim only | read-only routing diagnostic | `pij --help` ("pij generation [verb] [--json]"). It is not in RS_ROUTE_TABLE, so adding it to the table would **fail** the coverage test. |
| `bg emit` | shim + native | event-source fire, uses PIJ_BG_TOKEN | shim row `bg`/`emit`. The table leaf list `bg (create/list/tail/kill)` omits it; C7 does cover it. |
| `inbox check`, `inbox register` | shim | claim alias; external pull registration | `pij --help`. `register` is mentioned in prose (L41); `check` is not mentioned. |
| `inbox release` | native | park a running head after a failed consumption | `pij-rs inbox release --help`. Covered in peer.md, not in the table. |
| `ping` | native only | daemon health | `pij-rs ping --help`. Used in 00-routing signal D but absent from the table. |
| `doctor` (`claude-inbound`/`claude-hook`/`claude-statusline`) | native only | read-only audit of the Claude homes | `pij-rs doctor --help`. Absent from the whole skill. |
| `peers` (`check`/`new-key`) | native only | machine pairing (federation) | `pij-rs peers --help`, docs/how/pij-federation.md. Absent from the whole skill. |
| `register` | native only | seat-claim registration (`--harness --folder <id> --supersedes`) | `pij-rs register --help`. api.md calls it "native/HTTP-only". The table is silent. |
| `activity` | native only | busy/idle publication for turn-boundary hooks | `pij-rs activity --help`. Hook plumbing; could be listed as "hook-only, not agent-facing". |
| `fyi-claim`, `fyi-read` | native only | typed-turn hook FYI claim; read a claimed FYI digest in full (plan 159) | `pij-rs fyi-claim/fyi-read --help`. `fyi-read` is agent-facing (a digest names it). Absent from the skill. |
| `sidecar` (`telegram`/`bg-start`/`bg-cancel`/`chore-add`/`chore-run`/`chore-ack`) | native only | outbound Telegram; low-level bg; chore probes | `pij-rs sidecar --help`. `sidecar telegram` is the documented outbound path (docs/how/pij-telegram.md:10). |
| `daemon bounce` | native only | rebuild, drain, restart, verify, print version | `pij-rs daemon bounce --help`. The skill never names it (ops uses `just bounce-rs`). |
| `tail` (native) | native | daemon event NDJSON (`--since MACHINE=CURSOR`) | `pij-rs tail --help`. The table already notes "native tail is daemon events". OK. |

### B. Table verbs that no longer exist

None outright. Every table verb has an explicit shim row (served or a named refusal). These exist only as refusals, with no native counterpart:
- `agent` (all 8 leaves), `path`, `models`, `watch`, `unwatch`, `focus`: shim `E-RS-UNPORTED`; no `pij-rs` verb. `crates/harnesses/src/models.rs` is a catalog reader that nothing in daemon/cli calls.
- `sessions`: shim-only by design (api.md "no `pij-rs sessions` subcommand").

BLOAT note: the refusal-only verbs take 4 table rows (L45, L46 partly, L48, L53) plus registry row L32. They could be collapsed into one "named refusals" row without breaking the coverage test, which only parses backticked top-level verbs.

### C. Verbs marked E-RS-UNPORTED that now work (natively or via a substitute)

| table verb | shim | native reality | verdict |
|---|---|---|---|
| `telegram` (L46) | refuses | **`pij-rs sidecar telegram --body/--body-file [--chat-id]`** ships outbound and binds the seat as the reply target (sidecar telegram --help; docs/how/pij-telegram.md:10-13; inbound routes back as `pij-telegram` turns, sidecars/src/telegram.rs:539) | STALE: works natively |
| `daemon` (L46) | refuses | `pij-rs daemon` (run) + `pij-rs daemon bounce` (rebuild/drain/restart/verify) | STALE: "use documented native daemon lifecycle" never names it |
| `chore` (L53) | refuses | `pij-rs sidecar chore-add/chore-run/chore-ack` exist. api.md#unsupported-status: "does not remove already composed sidecar internals; it does not advertise those old administrative grammars as a native port" | UNVERIFIABLE whether agent-facing: needs a ruling (advertise or hide) |
| `tree` (L43) | refuses | `pij node show <seat>` returns the seat's governed work tree with children (governance.rs:1561-1573; `pij-rs node --help` "Read a seat and its governed work tree") | MISSING substitute pointer |
| `spawn` `revive` (L40) | refuses | native works; the table already says "use explicit native rs grammar" | OK |
| `tail` (L43) | refuses | native = events; the table already says so | OK |
| `models` (L48) | refuses | still no native verb or endpoint | OK (still unported) |
| `agent` `path` `watch` `unwatch` `focus` | refuse | no native | OK (still unported) |

Native flags the table/C1 never surface:
- spawn `--role <pm|worker|pa>`, `--model`, `--effort`, `--bin`, `--allow-retired`, `--id`, `--no-wait`, `--wait-seconds`, `--no-accept-inbound`;
- revive `--fresh`, `--assume-dead --evidence`;
- adopt `--role`, `--reclaim`, `--harness-session`;
- send `--to <seat>@<machine>`, `--force --reason`, `--msg-id`.

peer.md covers several of these; federation and `--force` are absent from core.

---

## skills/pij/SKILL.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 3 | description: "run pij agent packs (flowspace-search etc.)", "run flow-pair … fleets" | The `agent` CLI family refuses `E-RS-UNPORTED` (shim table; agent.md L1 "rs CLI gap"). No flow-pair skill is installed: `skills/flow-pair/` has no SKILL.md and is not in `~/.agents/skills`. The description still triggers the skill for these. | STALE |
| 24 | `pair` = "wrapping the-flow" | `the-flow` is now a redirect to `builder`. No `the-flow.json` exists under docs/plans (`find docs/plans -name 'the-flow*'` is empty); plan 173 uses `plan.dd.json`. | STALE |
| 26, 86 | `agent` registry row and alias "run flowspace search / ask an agent" → `/pij agent`, with no caveat | The route is a refusal shell. The `watch` row (L32) gets an "unported" note; `agent` doesn't. | MISSING caveat |
| 41 | send row lists `--fyi` only | `pij send --help`: the cold-wake guard refuses with `E-RS-COLD-WAKE`; the override is `--force --reason` (audited); the receipt reads `cold-check: …` | MISSING (peer.md has it; low) |
| 43 | `tree` unported | `pij node show <seat>` returns the governed child tree (governance.rs:1561-1573) | MISSING substitute |
| 46 | `telegram` → "shim E-RS-UNPORTED; use documented native daemon lifecycle" | Native `pij-rs sidecar telegram` ships outbound (docs/how/pij-telegram.md:10). Telegram is not a daemon-lifecycle matter. | STALE |
| 46 | `daemon` → "documented native daemon lifecycle" | `pij-rs daemon bounce` exists; no skill file names it | MISSING |
| 51 | `bg` (`create/list/tail/kill`) | Also `emit` (shim row; `pij-rs bg --help`) | MISSING (low) |
| 53 | `chore` "named E-RS-UNPORTED; no legacy escape" | Shim refusal is true. Native `pij-rs sidecar chore-add/run/ack` exists but is deliberately not advertised (api.md#unsupported-status). | UNVERIFIABLE (needs ruling) |
| 36-57 | the table is the "every `pij` verb has a home" inventory | Native-only verbs are absent: `ping` `doctor` `peers` `register` `activity` `fyi-claim` `fyi-read` `sidecar` `daemon bounce` `inbox release`. The coverage test would reject them as top-level rows (generation-skill-coverage.test.ts:27-35), and no check runs the other direction. | MISSING (structural) |
| 45,46,48,53 | four rows of refusal-only verbs | These could be one "named refusals" row | BLOAT |
| 62 | restates generation=legacy, raw `--json` envelope, fixtures path | Repeated nearly verbatim in 00-routing L5 | BLOAT |
| 66 | invariant 1: never write `.the-flow-state.json` / `the-flow.json` / `the-flow.md` | Builder now owns plan state (`plan.dd.json`, `assets/impl-guide.dd.json`). The forbidden list names files that no longer appear. | STALE |
| 77 | "the daemon's watchdog serves PAs only" | Opt-in watchdog for any seat (`pij-rs watchdog --help`; api.md#opt-in-watchdog; http/watchdog.rs). This contradicts L52 and C9 of the same skill. | WRONG |
| 77 | "2026-07-31 ruling at the resolved government root" | The government root is outside the repo; not resolved here | UNVERIFIABLE |
| 84 | `/flow-pair start\|dispatch\|…` alias → `/pij pair` | No `flow-pair` skill is installed (no `skills/flow-pair/SKILL.md`, not in `~/.agents/skills`). The alias can only fire if a user types it from memory. | STALE (low) |
| 92 | References: "Shared conventions (C1 … C7)" | 00-routing defines C1–C12 (C8 terminal, C9 watchdog, C10 wire, C11 trailers, C12 niced) | STALE |

Also MISSING (core is the router, so it should at least point to these):
- federation: `pij-rs send --to <seat>@<machine>` (send --help) and `pij-rs peers check/new-key`; docs/how/pij-federation.md exists. No skill file mentions it.
- the plan 173 prime-active PA wake. This belongs in C9; see below.

Checked OK (32), including:
- registry modules all exist (9/9);
- `watch` has no module;
- spawn/revive shim refusal plus native grammar;
- send, compact-self, adopt, whoami, state and inbox are served;
- the list filters (harness/folder/parent/scope=local, boolean `--here`);
- sessions has no filters;
- the `link` grammar and semantics (role_assertion.rs:246-267: no live parent, or re-role own child);
- the agent leaves are unported;
- phonehome, reap and close are served;
- close is self/parent only with no force flag;
- models is unported (catalog code unwired);
- commit-trailers is native-forwarded;
- `fleet-report` (plan 162: never contacts the daemon, store is read-only, `--anonymise` flag; fleet_report/mod.rs:1-8);
- the watchdog on/off/status/`--every` grammar;
- `E-RS-WATCHDOG-PA`;
- the orchestration baton/prime/role leaves, with return/reclaim taking `--lease-id` (governance.rs:445-453);
- role;
- the node-route verbs and leaves (task set/close, node show, report now/question/blocked/state/clear/verify, spine append/events/render; governance.rs:429-455, report --help);
- decisions/answer;
- the api.md anchors `#unsupported-status`, `#pa-watchdog` and `#opt-in-watchdog` resolve;
- `crates/testkit/fixtures/golden/api/` exists;
- the routing-enforcement test reads this table;
- `PIJ_DAEMON_GENERATION=legacy` refuses (generation-routing.ts:497);
- the protocol.md "Human rulings" heading and the batons.md and bootstrap §5 links resolve.

---

## skills/pij/references/00-routing.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 5 | Same content as SKILL.md L42/L62 (legacy forcing, raw `--json`, list/sessions filters, fixtures) | Duplicated prose | BLOAT |
| 14 | Signal A: newest `.flow-pair/runs/*/run.json` with `status: open` | The schema enum `open\|closed` is true (run.schema.json:18). No `.flow-pair/` dir exists in canonical, and the flow-pair skill is not installed, so whether anything still writes it is unverified. | UNVERIFIABLE |
| 16 | Signal C: newest `docs/plans/*/the-flow.json` → `harness flow nav show --path` | No plan has `the-flow.json` (`find docs/plans -name 'the-flow*'` is empty; 173 uses `plan.dd.json`, builder). `harness flow nav show` exists, but its input file never does, so the signal is dead. | STALE |
| 36-43 | C1 delivery table: rows for prereq/spawn/message/FYI/receive/compact/peek/retire | No **forced cold-wake** row. `pij send <id> --force --reason "<why>"` and `pij_send({…, force:true, reason})` exist (`pij send --help`; .omp/extensions/pij/index.ts:91), and `E-RS-COLD-WAKE` refuses waking sends to cold seats. The FYI row says "held", but the guard that makes FYI necessary is not mentioned. | MISSING |
| 37 | tmux spawn = `pij-rs spawn --harness <h> --cwd <abs> --parent <id> …` | True, but omits `--role <pm\|worker\|pa>` (stamps the role; caller becomes parent), `--model` and `--effort` (`pij-rs spawn --help`) | MISSING (low; peer.md has `--role`) |
| 38 | message = `pij send <id> '<text>'` | Native is `pij-rs send --to <id> --body …` (different grammar), and `--to <seat>@<machine>` (federation) is absent | MISSING (low) |
| 45 | "Existing-seat structural repair through `link` is unported, so never omit parent assuming that command can fix it later" | `link` shipped in plan 166: `pij link <seat> --role <r>` takes a seat with no live parent, or re-roles your own child (shim row; http/role.rs:615; store/role_assertion.rs:246-267; SKILL.md L44). It still cannot re-parent a seat whose parent is live, but "unported" is false. | WRONG |
| 93 vs 102-110 | "This applies to every layout (`window` names the window; `stack`/`right`/`below` name the pane title)" | The legacy `--layout` values; L93 says native spawn doesn't accept them | STALE |
| 145 | "spawn with --layout window, then move" | `pij-rs spawn --help` has no `--layout`. This contradicts L93 and L153 ("do not pass an unported layout flag"). | WRONG |
| 95-112 | Naming mandate: rename the window after spawn | MISSING: `pij-rs spawn --name <window>` sets the window name at launch (it defaults to the seat id). An encodable half of the mandate goes unmentioned. | MISSING |
| 163 | C6: "Check the installed `pij-rs daemon --help`" for lifecycle | `pij-rs daemon bounce` (rebuild/drain/restart/verify) and `just bounce-rs` (justfile:564, which also defaults `PIJ_RETIRED_HARNESSES=pi`) go unnamed | MISSING |
| 202 | "`pij send <self>` is still E-SELF" | rs refuses self-sends with kind `refused`: "a seat cannot send to itself — use `pij compact-self`" (core/error.rs:140; core/delivery.rs:315; daemon/http/mod.rs:2032). `E-SELF` is the legacy TS code (.omp/extensions/pij/core/types.ts:602). The behaviour is true; the code name is wrong. | STALE |
| 206 | C8: "persisted pij-owned close is **requested**; observed absence … **unrequested-by-pij**; … boot reconciliation **historical**" | No such classification in crates/: grep for `unrequested`/`no-show` is empty, and `historical` appears only in a registration comment. These are TS concepts (.omp/extensions/pij/core/spawn-expectation.ts). | STALE |
| 208 | "Each launch has a bounded expectation keyed by `spawnId` … descriptor with same key suppresses the no-show" | rs has no no-show notice. Spawn waits `--wait-seconds` (default 30) for registration, and the spawn id is the sole registration proof (commit 314c33c, registration.rs:188). | STALE |
| 204-208 | C8 (what C8 should say) | MISSING: the rs death sweep (every 5s, `PIJ_RS_DEATH_SWEEP_MS`) tombstones observed-dead seats with `reason: "observed-dead"` and pushes an obituary from `pij-bg` to the recorded parent: `[pij] seat <id> died … revive with pij-rs revive <id>`. Parents need not poll (death_sweep.rs; api.md#death-sweep). | MISSING |
| 212 | C9: PA nudged at the fleet's idle edge and once at a usage-limit reset | Also **wakes the PA once when its prime resumes after the idle edge** (plan 173, merged at HEAD: 8114b28/bb9f0b1; api.md "Prime-active wake"; docs/how/pa-watchdog.md). Not mentioned anywhere in the skill. | MISSING |
| 216 | C10: "Evidence: plan 083 (~2.9M tokens …)" | This checkout's docs/plans contains only 173, so the citation can't be resolved here | UNVERIFIABLE |
| 248 | C12: the "2026-09-29" load ~230 incident | Policy/anecdote; no artifact in repo | UNVERIFIABLE |

Checked OK (58), including:
- signal B, using `pij state <id> --json`;
- signal D, `pij-rs ping --json`;
- signal E/F ordering, matching the Copilot `pij_send` with `fyi` in `.copilot/extensions/pij/extension.mjs`;
- the C1 adopt form (`pij-rs adopt [PANE] --harness --parent`, where "omitted parent = UNSAID", per adopt --help);
- the identity env vars (`CLAUDE_CODE_SESSION_ID`, `COPILOT_AGENT_SESSION_ID`, `CODEX_THREAD_ID`, from CALLER_ENV_ALLOWLIST) and `~/.copilot/session-state` (daemon/lib.rs:1098);
- `inbox register` → `/v1/register`;
- `pane:null`/`native_extension_delivery` (model.rs:437);
- the inbox JSON `data` being a claims array;
- `pij send --fyi`, and `--command compact` being accepted by the shim parser (http/shim.rs:269-327);
- C2 `pij canary --expect-model` (governance.rs:440);
- C3 compact-self has no args/`--pane`;
- C4 models unported and the peer.md anchor resolving;
- C5 native spawn `--session/--name/--cwd/--parent`;
- C6 `pij daemon` shim refusal;
- the C7 bg grammar:
  - flags: `--title/--command/--cwd/--timeout/--events/--fyi/--min-interval/--inline-max`, `list --all`, `tail --lines`, `kill`, `emit --data/--data-file`;
  - details: 20 inline lines (background.rs:1180), log mode 0600 (background.rs:379), the 10,000 pending cap (`PENDING_CAP`), `POST /v1/bg/{job}/emit` (http/background.rs:22);
  - cold owner → prime/Telegram (background.rs:90-97);
- C9:
  - PA-only default and `E-RS-WATCHDOG-PA`;
  - the opt-in nudge text including `pij watchdog off` (core/watchdog.rs);
  - old leaves refusing `E-RS-ARG`;
  - `report state done` being a valid word (report --help: ready|waiting|hold|blocked|question|done|failed|cancelled);
- C11:
  - native forwarding;
  - the Pij-Seat/Pij-Prime/Pij-Plan keys;
  - no designation lookup, since `derive(…, None, …)` at commit_trailers.rs:58;
- the anomalies `--project/--here` filters exist (http/anomalies.rs:245), which is why invariant 12 says to query unscoped.
