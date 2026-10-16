# Skill survey — partition `prime-rituals`

Scope: `skills/pij/references/prime/{rituals,templates,exemplars}/` @ bb9f0b1 (13 files, 767 lines), all read in full.
Ground truth: governance verbs are forwarded unparsed to the daemon, so `pij-rs <verb> --help` shows nothing for them. Their grammar is the single parser in `crates/daemon/src/http/governance.rs:428-455` (the allowed positionals and flags per leaf). Role and link: `crates/daemon/src/http/role.rs`. Watchdog: `crates/daemon/src/pa_watchdog.rs` and `docs/how/pij-rs-api.md` §PA watchdog.

## Summary (ranked)

1. **WRONG, store-native.md:16-20**: it says `--project` on `task set` is inherited by later "state-set/state-verified" events, so they show up in the project-filtered spine. They don't. The events are `report.state` and `state-verified`, and their payloads carry `assignment_id`, never `project` (`crates/core/src/report.rs:45-58`, `crates/store/src/decisions.rs:284-292`). `spine events --project` matches only `payload.project`, `record.project`, project-created/set slugs or a `project:<slug>` ref (`governance.rs:1492-1504`). The paragraph also sits inside the mapping table, so rows 21-23 lose their header and render broken.
2. **WRONG, link is "unported"** (kickoff.md:22 and :26, store-native.md:22). `pij link <seat> --role <pm|worker|pa>` shipped in plan 166 (`role.rs:556-650`, route `/v1/link`; pij-rs-api.md:660, :934). bootstrap.md:58 already uses it, so the files contradict each other. "Existing-seat reparenting is a named gap" is only partly true now: link takes a seat whose parent is absent or not live.
3. **WRONG, stream-brief.md:65**: role `stream-s<ORD>`. The role vocabulary is closed (`prime|pm|worker|pa`, `crates/core/src/orchestration/governance.rs:161-165`). `pij role` refuses anything else with E-RS-ARG.
4. **STALE, bootstrap.md:46**: the pij worked example of the government root no longer resolves. The canonical checkout's origin is `AI-Substrate/pij-fleet`, not `AI-Substrate/pij`. `$HOME/pi-hacking/pij-worktrees/pij-governance` does not exist. `harness/scripts/government-root.sh` exits 1 ("found 0"). `prime-governance` exists only in `pij-legacy` (origin AI-Substrate/pij), with no worktree registered anywhere and no such head on pij-fleet's origin. Every ritual that writes to `<government-root>` (canaries, incidents, handover, orient-local) has no target on this machine.
5. **STALE, kickoff.md:51-55** (the Canary table): it describes a hand-rolled nonce send and a text `brief-ack`. `pij canary <id> [--expect-model] [--wait ms]` now runs leg (a) and leg (b) mechanically: it mints a nonce packet, dispatches it, waits for `pij ack`, then checks process, pane, session and model. It refuses with `E-RS-CANARY-PENDING` (default wait 5s) or `E-RS-CANARY` (`governance.rs:1793-1955`). Leg (c) closes on dispatch state `acked` from `pij ack`, not on a free-text `brief-ack`.
6. **STALE/MISSING, bootstrap.md:61**: it says the PA's only clocks are the idle-edge nudge and the usage-limit wake. It omits the plan-173 **prime-active wake**, merged at HEAD in PR #53 (`pa_watchdog.rs:23-33`, pij-rs-api.md:835-842).
7. **STALE, stream-brief.md:54**: `WAITING_FOR_BUILD_CONFIG` comes from the-flow era. It doesn't appear anywhere in the `/builder` skill (`~/.agents/skills/builder/**`) or in the installed harness CLI.
8. **MISSING, kickoff.md**: it never names the shipped `pij fence set <stream> --paths a,b [--shared ..]` / `fence show [--stream|--path]` (step 3), `pij stream close <id>` or `pij task close <id> --reason done|cancelled|failed|superseded` (step 15), or `pij-rs spawn … --role pm` (step 7/10, which makes the separate `pij role` call unnecessary).
9. **Minor**: `the-flow archival` (bootstrap:36). "tree-push channel" (incidents:38). Copilot is missing from the extension-registration list (bootstrap:10, though seat-handover:11 has it). E-RS-PIN is unnamed and grant/reclaim are not keeper-gated (batons). "Vendored war stories N" are cited but nowhere in the repo (exemplars). The "Cause and expiry" section in batons is BLOAT.

---

## rituals/bootstrap.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 10 | "OMP and Pi each register through their runtime extension at boot" | Native Copilot (`pij_send`) is also extension-owned (00-routing C1:30). seat-handover.md:11 already says "OMP, Pi and Copilot". | STALE |
| 36 | `docs/plans/<ord>-<slug>/` stays on main "(code PR + the-flow archival)" | Plans now go through `/builder`. Archival is `7b post-flight`, which moves the folder to `docs/plans/archive/` (builder SKILL.md:47,53). | STALE |
| 43 | `harness flow create prime-flow … --path <government-root>/prime-flow.json` | Flags exist (`harness flow create --help`), but `--path` "must be in-repo". It works only if run from inside the governance worktree; the text doesn't say so. Not run (it mutates). | UNVERIFIABLE |
| 46 | "`AI-Substrate/pij` uses `$HOME/pi-hacking/pij-worktrees/pij-governance`; resolver `harness/scripts/government-root.sh`" | `git remote -v` gives pij-fleet. The path is absent. The resolver exits 1 "found 0". `prime-governance` exists only in `~/pi-hacking/pij-legacy` (local and remote), with no worktree registered. `git ls-remote --heads origin prime-governance` on pij-fleet returns nothing. | STALE |
| 61 | PA clock = one idle-edge nudge plus one usage-limit reset wake | Also a prime-active wake: once per worker-idle episode, when the prime starts working on its own (plan 173; `pa_watchdog.rs:23-33`; api.md:835). | STALE |
| 78 | "Stream dies → explicit native revive/spawn" | True, but doesn't say that `pij revive` (shim) refuses and only `pij-rs revive` works; native revive resumes the conversation (plan 156, api.md Revive). | MISSING (minor) |

17 claims checked OK, including: the `pij project list --json` and `pij list --json` refusals of `--prime`/`--archived` (shim help); no prime getter (api.md:932; no GET route for designation in `http/mod.rs`); `pij whoami --json`; `pij orchestration prime set <id>` returning `{prime,seq}` (`governance.rs:1262-1297`); `spine events --peer` finding prime-set (event seat = designated seat); `pij-rs spawn --role pa`; `pij link <id> --role pa`; role arming the PA watchdog; fleet by repository; first look only arms, supervisors never count; the `watchdog.round` event; the resolver algorithm text matching the script; `/builder 8 ship`; the anchor `#governance-branch--the-rules`; the links to orient-oprime, orient-global, pa, templates and kickoff#canary.

## rituals/kickoff.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 10-11 | "Derive descriptive fences from actions" (no verb) | `pij fence set <stream> --paths <csv> [--shared <csv>]` and `pij fence show [--stream] [--path]` shipped (`governance.rs:436-437, 880-926`). Fences are store records with a `fence` event. | MISSING |
| 18 | spawn line has `--parent` but no role | `pij-rs spawn --role <pm|worker|pa>` stamps the role and makes the caller the parent (spawn help), which replaces step 10's separate `pij role` call. | MISSING |
| 22 | "tree/link remain unported" | `tree` is still unported (shim help). `link` is ported (plan 166): `role.rs:556-650`, api.md:934 "`link` is ported". | WRONG |
| 26 | "existing-seat reparenting is a named gap, not a role side effect" | `pij link <seat> --role r` takes a live seat whose recorded parent is absent or not live and writes `seat.put` and `role-set` together (api.md:660). Reparenting away from a *live* parent is still impossible. | STALE |
| 23-25 | `pij canary <id> --expect-model <m>`, then "complete leg (a) round-trip and leg (b) identity proof" by hand | The verb does (a) and (b) itself, with a nonce dispatch, ack wait and process/pane/session/model observation (`governance.rs:1793-1955`). The default `--wait` is 5000 ms (max 300000), which booting peers need to know. | STALE |
| 51-55 | Canary table: (a) "send a nonce challenge; ack arrives as injected turn"; (b) "capture pane footer as fallback"; (c) "require `brief-ack`" | (a) is now automated by `pij canary`. (b) fails closed: `E-RS-CANARY` if the session is unbound or the model can't be observed. A footer read is an out-of-band supplement, not a pass. (c) is `pij ack <dispatch> --packet-sha`, giving dispatch state `acked` (visible in `pij node show`). "brief-ack" is legacy wording. | STALE |
| 27-29 | `pij dispatch <id> --packet <brief> --wait` | Grammar is OK. But bare `--wait` is 5000 ms and returns at `delivered` *or* `acked` (`wait_dispatch(…, ack_only=false)`, `governance.rs:1678, 1771-1791`), so the receipt doesn't prove the ack. The text half-implies it does. | MISSING (clarify) |
| 35 | Retirement: "retire allocation/history without erasure" | The verbs are `pij stream close <project:slug>` (record only; api.md:669) and `pij task close <id> --reason done|cancelled|failed|superseded`. Neither is named. | MISSING |
| 8 | `pij stream create --project <p> --slug <s> [--base <ref>] [--ordinal N]` | OK. `--root` (worktree root) is also accepted (`governance.rs:433`). | MISSING (minor) |

16 claims checked OK, including: stream create owning the worktree via `git worktree add -b` (`daemon/src/orchestration/repo.rs:90`); `pij-rs spawn --harness --model --effort --cwd --parent --name --json`; no `--layout`/`--task`; shim spawn refusing `E-RS-UNPORTED`; native waits by default with `--no-wait`; `pij list --json` / `pij node show <id> --json`; `pij role <id> pm --json` grammar (`role.rs:471-512`); `pij ack <id> --packet-sha` and that the pushed body names it (`governance.rs:1726-1729`); `pij close` tombstones without touching the process or pane; the fixture paths `governance-routes.json` and `governance-events.json` exist; the api link resolves; the C11 and protocol § refs exist.

## rituals/batons.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 33-35 | grant "stale/unverifiable requires explicit `--repin`" | Correct, but the refusal code `E-RS-PIN` (`governance.rs:1172`) isn't named, while `E-RS-LEASE-STALE` is. | MISSING (minor) |
| 33, 41 | Keeper grants; reclaim is explicit | The code does **not** restrict grant or reclaim to the keeper (`created_by`). Any resolved seat can grant a request, or reclaim with the lease id and `--evidence` (`governance.rs:1151-1261`). Only return is holder-only. "The primitive records, never decides" is consistent with that, but readers should be told grant authority is by protocol only. | MISSING |
| 75-89 | "Cause and expiry" section (koala, araminta, pij#72, stale badges, PA exemptions) | 15 lines of general doctrine and war story, not baton mechanics. "PA exemptions" refers to the retired TS watchdog `exempt` leaf, which now refuses (api.md:934). Exempt seats survive only as a pa.md brief slot. | BLOAT |

20 claims checked OK. Leaves define, list, show, request, grant, return and reclaim all have the stated flags (`governance.rs:445-450`). Also OK: request returns `{request,seq,notice}`; the notice goes to `created_by`; `unverified` comes from Dead/Recycled; a tombstoned or missing keeper gives `null`; Held/Refused give `E-RS-PARTIAL` with committed/event_published/request/seq (`governance.rs:1116-1149, 1346-1389`); return is holder-only and reclaim needs `--evidence`; state and event commit atomically (`publish_committed`); `E-RS-LEASE-STALE` names the current lease and the fix `pij orchestration baton show <name>` (`governance.rs:1391-1402`); there is no auto-reclaim; `DeliveryOrigin::ReaderRead` exists (`core/src/model.rs:790`); the api anchor `#baton-leases` exists (api.md:774).

## rituals/reports.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 1-20 | Reports are evidence files plus a pointer send | Never mentions the store-side duties: `pij report now/state` cards (SKILL invariant 12) and the parent's `pij report verify <seat>` (parent-only, binds the latest done seq; `decisions.rs:298-331`). A receiver's "verification note" (line 26) has a mechanical store receipt it doesn't cite. | MISSING |

5 claims checked OK: the C10 and C11 anchors exist in 00-routing.md (lines 214, 228); the government-root rules anchor resolves.

## rituals/incidents.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 38 | broadcast "(tree-push channel)" | `pij tree` is unported (shim help: E-RS-UNPORTED). The channel is ordinary sends or FYIs. | STALE (minor) |

4 claims checked OK (anchors `bootstrap.md#recovery`, batons.md and reports.md links resolve; the record path is under `<government-root>`, but see bootstrap:46 on resolvability).

## rituals/store-native.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 16-20 | `--project` on `task set` → "every later state-set/state-verified event inherits it", else orphaned from the filtered spine | The state events are kind `report.state` (payload `StateRecord{state,note,assignment_id,refs,registry_seq}`) and verify is kind `state-verified` (payload `{seat,verified_by,done_seq,assignment_id}`). Neither carries `project`. The spine `--project` filter can't see them either way (`governance.rs:1492-1504`). Only the `task-set` event carries the project. | WRONG |
| 16-20 | Paragraph placed between table rows | Rows 21-23 (verification, seat/liveness, render) render as broken pipes with no header. | WRONG (format) |
| 22 | "tree/link are unported" | link is ported (plan 166). See kickoff:22. | WRONG |

15 claims checked OK, including: append grammar `--kind/--refs/--project` with no `--peer`/`--bare` and `--actor` checked against the caller (`governance.rs:454, 543-550`); events/render use `--since/--peer/--project`; render returns `{text,cursor}` (`governance.rs:1511-1524`); `project create "<desc>"` slugifies the description (`governance.rs:666-723`); `project set <slug> --plan --prime`; `task set <id> "<task>" --project`; `report state` and parent-only `report verify` (`decisions.rs:313`); `anomalies --json`; the fixtures dir and the Hello `{build,hello:true,v:1}` (`governance-events.json:5-7`); prime designation and leases are separate records.

## templates/stream-brief.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 54 | "A validated plan stops at `WAITING_FOR_BUILD_CONFIG`" | No such state in `~/.agents/skills/builder/**` or the installed harness CLI. It survives only in pij's own prime docs (orchestrator.md:42, protocol.md:55). This is the-flow era. | STALE |
| 65 | identity "role `stream-s<ORD>`" | The closed vocabulary `prime|pm|worker|pa` rejects it (`core/src/orchestration/governance.rs:161-192`; api.md:657). A stream seat's role is `pm`. | WRONG |

9 claims checked OK (`/pij prime` routes to orchestrator.md (routes/prime.md:20); `/thesis` and `/builder 8 ship` exist; the C10 ref exists; the government-root anchor resolves).

## templates/seat-handover.md

No defects. 10 claims checked OK: Copilot, OMP and Pi extension registration and C1 exact-pane adopt; `pij whoami --json`; `orchestration prime set` and `prime retire <id> --json` returning `prime.state` = `retired` (serde lowercase, `core/src/orchestration/governance.rs:195-202`); `spine events --peer`; no `list --prime` or tree projection; the C10 ref; the store-native link.

## templates/orient-local.md, templates/spine.md, templates/baton-book.md

No defects. orient-local: `harness observe "<what>" --kind <kind>` matches `harness observe --help`, and `/builder 8 ship` exists. The spine and baton-book templates are prose-only for unmigrated primes, which bootstrap.md:42 states correctly. 6 claims checked OK.

## exemplars/canary-record.md, exemplars/grant-log.md

| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| canary-record:24, grant-log:28,33 | "Source: vendored war story N" | No war-story file anywhere in the repo (`grep -r "war stor"` hits only these files). The citation dangles. | UNVERIFIABLE |

4 claims checked OK. Both are labelled historical. The disclaimers are accurate: native spawn has no `--layout`, and the shim tail/list don't reproduce the old rows.
