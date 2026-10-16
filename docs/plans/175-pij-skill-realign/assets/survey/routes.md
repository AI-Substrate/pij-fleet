# Partition `routes`: skills/pij/references/routes/*.md (main @ bb9f0b1)

## Summary, worst first
1. **WRONG, ops.md:58, the reboot procedure.** It says revive "is not the former `--print`, `--attach` or `--assume-dead` workflow, and requires a known tombstoned id". In fact `--assume-dead --evidence` is a live native flag, and revive also accepts an untombstoned seat seen as `Dead` with its pane absent (revive.rs:303-399; `pij-rs revive --help`). This contradicts peer.md:43/54 in the same skill.
2. **MISSING in ops.md and peer.md, revive resume semantics (plan 156, in revive.rs since 03ab38e).** Revive resumes the recorded `harness_session`. `--fresh` is the only way to launch a blank conversation; without a recorded session, revive refuses `E-RS-ARG`. `E-RS-REVIVE-CONVERSATION-LIVE` means "adopt that pane instead". Neither route mentions any of this, and it is the core of after-reboot recovery and the double-seat issue (#55).
3. **WRONG: link is described as unported, in pair.md:13, pair.md:154 and node.md:101.** It was ported in plan 166: `pij --help` forwards `link` to `/v1/link`, and role.rs:555 parses `link <seat> [--parent <you>] --role <pm|worker|pa>`. peer.md:25 and prime.md:49 already document it correctly, so the skill contradicts itself.
4. **WRONG, node.md:12, `pij role <seat> reviewer`.** `reviewer` is refused. SEAT_ROLES is `prime|pm|worker|pa` (core/orchestration/governance.rs:161).
5. **WRONG, ready.md:22, "`pij adopt` rejects `--harness pi` by design".** The daemon accepts it: identity_tests.rs:1310 expects 200 for `pi`. The rule not to adopt Pi still holds, but the stated reason is false.
6. **WRONG/STALE, pair.md fleet machinery.** The `run.json` roster (`role → {pijId,paneId,model,spawnedByUs}`) does not exist anywhere in skills/flow-pair. The route also forbids writing the ledger by hand, so its instruction cannot be followed. `start` ignores `"<request>"`, `--coder-model` and `--reviewer-model`. flow-pair is not installed as a sibling skill in ~/.agents/skills or ~/.claude/skills; it exists only at repo `skills/flow-pair`. The route wraps "the-flow", which is now a redirect to /builder. `.the-flow-state.json` no longer exists; `the-flow.json` is written only by the `harness flow` CLI.
7. **STALE, other claims.** ops.md:58 calls in-place reattachment a "named gap", but `adopt --reclaim` and conversation-following adopt exist (plan 156). ready.md:34 says revive needs a tombstoned seat. pair.md:182 mentions a `stalled` push, which rs does not have. prime.md:59 says "no acks or FYIs", but held `--fyi` has shipped. The pair.md default models (`claude-sonnet-4.6:xhigh`, `gpt-5.5:xhigh`) use Pi-style suffixes, not native `--effort`.
8. **Revive code gap, relevant to #55 (in flight: PRs #56/#57 open, not merged).** On the tombstoned path, revive skips the process and pane veto entirely. `conversation_live_elsewhere` excludes the seat itself and, for non-Claude harnesses, sees only other *registered* live seats. So a `close`d but still-running OMP, Codex or Copilot seat, or one a human resumed by hand without registering, can be revived into a double seat. Claude is covered by the session-record scan.
9. **MISSING, smaller items.** The reincarnated-pane reap rule (the key fact after a reboot) is absent from ops.md. Also absent: `pij-rs daemon bounce`, revive `--fresh` (peer.md), the bg `emit` and `--events` options (peer.md:92 lists only create/list/tail/kill), and the FYI warm flush plus `pij-rs fyi-read` digest from plan 159. The pij_spawn examples in delegate.md and skill.md omit `--role worker`.
10. **BLOAT.** peer.md:77 restates the FYI rule word for word. That rule is also in `pij send --help` and in C10, which says "never restate".

---

## peer.md
| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 42-46 | Revive grammar is `<seat> [--session] [--name]` plus `--assume-dead --evidence`; "--print/--attach unsupported" | `pij-rs revive --help` also lists `--fresh`. revive.rs:108-145 resumes the recorded `harness_session`, refuses `E-RS-ARG` when none is recorded, and refuses `E-RS-REVIVE-CONVERSATION-LIVE` (with "adopt there instead") when the conversation is live elsewhere. The route mentions none of this. | MISSING |
| 54 | "An `Active` process or a present recorded pane still vetoes revival" | revive.rs:303 also vetoes an *unverifiable* pane (`pane_present != Some(false)`). That check applies only on the untombstoned path; a tombstoned seat gets no process or pane veto at all (revive.rs:245). | MISSING (partial) |
| 54/147 | The reboot row says only "tombstone or Dead+pane absent" | It should say that revive resumes the conversation and that you must check for a hand-resumed copy first (#55). See the ops section. | MISSING |
| 77 | The FYI rule, word for word | The same text is in `pij send --help`, `pij-rs send --help` and C10. C10 says "never restate". | BLOAT |
| 77 | The FYI mechanics | Correct as far as it goes, but it omits the plan 159 warm flush at 5 (`FLUSH_AT`, core/fyi.rs:27) and the digest that names `pij-rs fyi-read` (pij-rs-api.md FYI table; `pij-rs --help` lists fyi-read). | MISSING |
| 92 | "`pij bg create/list/tail/kill`" | `pij-rs bg --help` also has `emit`. `bg create` has `--events/--fyi/--min-interval/--inline-max/--timeout/--cwd` (plan 163). | MISSING |
| 48 | "This machine's `just bounce-rs` opts into `PIJ_RETIRED_HARNESSES=pi`" | True (justfile:573). `just rs-autostart` also sets it (justfile:601). | OK (note) |

Checked OK against help or code: adopt flags; list filters and `--here`; refused `--role/--prime/--archived`; `role --unset`; link semantics and refusals (role.rs:347-372); the paneless adopt refusal; all spawn flags (`pij-rs spawn --help`); OMP `--thinking` and Pi `model:effort` (harnesses/spawn.rs:162,169); the retired message (http/mod.rs:1066); the pij_spawn "spawned, NOT registered" text and the boot-failed message (rust-runtime.ts:84,467); `/health retired_harnesses` (rust-runtime.ts:632); the revive assume-dead authorization and `E-RS-REVIVE-LIVE` text; the legacy-tombstone event; the models, focus, watch and tail refusals (`pij --help`); every send form, including `--fyi`, `--force --reason`, `--command`, `--body-file` and native `--to` (shim.rs:230-340); the fyi+command refusal (shim.rs:343); cold-wake at 400k and 1h (core/cold_wake.rs:20,23); `E-RS-CONTROL-UNSUPPORTED` text (core/control.rs:10); the recovery env vars and defaults (10s, 2s, 120s, 20s heartbeat, 3 resends, 60s lease; rust-runtime.ts:135-143, delivery/mod.rs:299); the `inbox --peek` and `inbox release` flags; native tail `--since`; close and reap; death sweep at 5s, `PIJ_RS_DEATH_SWEEP_MS` and BG_ACTOR (death_sweep.rs:18-36). All links resolve. **~45 claims checked OK.**

## pair.md
| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 13 | "Shim spawn/agent/tree/link/models are named refusals" | `link` is not refused. `pij --help` shows "pij link: POST /v1/link". Plan 166. | WRONG |
| 154 | "Existing-seat reparent/root placement (`link`) is unported" | Link is ported (role.rs:555-650). The preferred path is now `pij-rs spawn --role worker`, which stamps the role in the spawn transaction (peer.md:25, prime.md:49). | WRONG |
| 6-11, 25-27 | Wraps `the-flow`; "`the-flow` guided mode is the sole writer of `.the-flow-state.json`, the-flow.json, the-flow.md" | the-flow is a redirect to /builder (~/.agents/skills/the-flow/SKILL.md). builder SKILL.md:101 says "There is **no `.the-flow-state.json`**"; the-flow.json is mutated only by the `harness flow` CLI. Product state now lives in plan.dd.json and impl-guide.dd.json. | STALE |
| 13-21, 250 | `<flow-pair skill root>` = "the INSTALLED flow-pair skill … a sibling of this pij skill in the skills install dir" | No flow-pair in ~/.agents/skills or ~/.claude/skills. It exists only at repo `skills/flow-pair/` (the real sibling of skills/pij after the symlink). No `just` recipe installs it. | STALE / risk |
| 132-134 | The roster lives in the ledger `run.json` as `role → {pijId,paneId,model,spawnedByUs}`, persisted by P9 | `grep -r roster skills/flow-pair` finds nothing. The CLI has no roster write path, and line 16 forbids writing the ledger by hand, so the instruction cannot be followed. | WRONG |
| 136, 203 | `--coder-model` / `--reviewer-model` on `pair start "<request>"` | `lib/cli.ts` HELP for start: only `--repo`, `--ledger-root`. The parser silently ignores both the unknown flags and the positional request (cli.ts:90-117). | WRONG |
| 141-142 | Default models `github-copilot/claude-sonnet-4.6:xhigh`, `github-copilot/gpt-5.5:xhigh` | The `:xhigh` suffix is Pi's translation; native spawn takes `--model` plus `--effort`. A memory note says gpt-5.5 is non-reasoning on Copilot. The defaults are older than peer.md's examples. `pij models` is unported, so this can't be checked through pij. | STALE / UNVERIFIABLE |
| 160 | "`pij state` shows it stuck `pending`/`failed`" | StateCard (http/types.rs:482+) has `state`, `badge` and `liveness` (active/dead/recycled). No pending/failed binding field was found. | UNVERIFIABLE (likely TS-era) |
| 182 | "on the colleague's done-report (or a `stalled`/`dead` push)" | rs has no stalled push (grep `stalled` in crates/daemon: none). `dead` is the death-sweep obituary. | STALE |
| 196 | "`accept` is unimplemented (a stub)" | Confirmed (cli.ts:49). | OK |
| 190-191 | Zero findings → refuses to mint a verdict | Confirmed (review.ts:217-223). | OK |
| 204-210 | dispatch, review, fix and learn flags | Match cli.ts HELP and handlers (`--delegation-id` or `--delegation`, `--phase-dir`, `--review-id`, `--miss-type`). | OK |

**~14 claims checked OK.** A mismatch belonging to another partition: pair.md:150 says "Native CLI has no legacy layout flags", but 00-routing.md:145 (C5) still says "spawn with --layout window".

## ops.md
| line | claim (short) | reality + evidence | verdict |
|---|---|---|---|
| 58 | Revive "is **not** the former `--print`, `--attach` or `--assume-dead` workflow, and requires a known tombstoned id" | `--assume-dead --evidence` is current (`pij-rs revive --help`; revive.rs:207-219, 304-341). Untombstoned `Dead`+pane-absent seats revive without a tombstone (revive.rs:301-399; pij-rs-api.md § Revive). This contradicts peer.md:43/54. | WRONG |
| 50-58 | The reboot procedure: re-observe, ping, then `pij-rs revive <tombstoned-seat>` | **The procedure needs these steps from the code.** (a) On restart, the death sweep tombstones each seat with a dead pid and an absent *or reincarnated* pane, as `observed-dead`. Obituaries to parents that are also dead are withheld (death_sweep.rs; reaper.rs:17,82). (b) Before reviving, check for a copy someone resumed by hand. For Claude, revive refuses `E-RS-REVIVE-CONVERSATION-LIVE` and names the pane. In that case, adopt that pane (`pij-rs adopt <pane> --harness claude`), or let the status line self-heal; don't relaunch. (c) Revive resumes the recorded conversation (`claude --resume`, `omp --resume=`, and so on). `--fresh` is the only blank launch, and a seat with no recorded conversation refuses `E-RS-ARG`. (d) Revive uses `no_wait:true` (revive.rs:419), so canary afterwards. | MISSING |
| 58 | Double-seat risk (#55) | Not mentioned. On main, `conversation_live_elsewhere` (revive.rs:147-197) checks other *registered* seats with a live process, plus, for Claude only, `<claude home>/sessions/<pid>.json` with a verified start. Non-Claude copies resumed by hand without registering are not detected. The tombstoned path never checks the seat's own process, which `close` leaves running; the loop filters `seat.id != prior.id`. #55 is OPEN, and PR #56 (`E-RS-CONVERSATION-LIVE` for stray panes, a one-time notice, status-line `⚠ double`) and draft PR #57 are not merged. Neither is shipped. | MISSING (note in-flight) |
| 58 | "In-place reattachment … remain[s a] named gap" | `pij-rs adopt --reclaim <seat>` (parent or prime) reattaches a dead seat to a pane. Adopt also follows a conversation and lifts its tombstone (`pij-rs adopt --help`; pij-rs-api.md § Adopt continuity order; identity.rs:275,337). | STALE |
| 46 | The reap eligibility rule | It omits the `reincarnated` pane rule: a pane whose root process started after the seat's process counts as absent (reaper.rs:17,82; reaper_tests.rs:650). That is exactly the post-reboot case. | MISSING |
| 9-14 | Daemon lifecycle: `pij-rs ping`, `pij-rs daemon --bind` | `--bind` exists (cli/main.rs:69-73). It doesn't mention native `pij-rs daemon bounce` ("Rebuild, drain, restart, verify health"), `--offline`, `--insecure-bind`, or `pij-rs peers` (federation). | MISSING |
| 75-77 | Government example `$HOME/pi-hacking/pij-worktrees/pij-governance` | The path doesn't exist. `harness/scripts/government-root.sh` reports "found 0" prime-governance worktrees, and the pij-fleet repo has no `prime-governance` branch (OSS cutover). | STALE / UNVERIFIABLE |

Checked OK: the daemon, path and telegram refusals; `PIJ_RETIRED_HARNESSES` and `/health`; the fixture `governance-routes.json` `isolation` key; the read verbs; the list filters; close semantics (lifecycle.rs:101-118,297-325); reap dry-run and the brake; the ps-absence protocol and both named tests (proc.rs:440,461); bg and controls. All links resolve. **~20 claims checked OK.**

## delegate.md
| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 23-24 | The forbidden list includes `.the-flow-state.json`, the-flow.json, the-flow.md | `.the-flow-state.json` is legacy (builder SKILL.md:101). The current plan state also includes `plan.dd.json` and `assets/**.dd.json`. | STALE |
| 36 | `pij-rs spawn … --parent <your-seat>` | It omits `--role worker`. prime.md:48 says to "stamp every … worker … on the placement call", and peer.md:25 says roles are never backfilled. | MISSING |

Spawn, send, close and push claims checked OK. **~8 OK.**

## ready.md
| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 22 | "`pij adopt` rejects `--harness pi` by design" | identity_tests.rs:1310 adopts `pi` with 200. The refusal text at identity.rs:637 lists `claude\|copilot\|codex\|omp` only as guidance, and `Harness::parse` accepts pi. | WRONG |
| 34 | "native revive requires an explicit tombstoned seat" | It also accepts untombstoned `Dead`+pane-absent seats (revive.rs:301). | STALE |
| 20-26 | Claude seats run `pij adopt` | Works. It doesn't mention that Claude's managed status line self-adopts and that adopt follows the conversation (pij-rs-api.md § Adopt continuity). That is harmless, but it explains `⛓ registering…`. | MISSING (minor) |

The inbox register, wait and adopt forms match `pij --help`. **~9 OK.**

## node.md
| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 12 | `pij role <seat> reviewer --json` | Refused: SEAT_ROLES = prime, pm, worker, pa (core/orchestration/governance.rs:161; role.rs:160). | WRONG |
| 101 | "`link`/`tree` are unported … reparenting/root placement is a named gap" | Link is ported (plan 166; role.rs:555; pij-rs-api.md Unsupported status: "`link` is ported (plan 166)"). `tree` is still refused. | WRONG |
| 65 | "Use `--note` with question/blocked" | `report question\|blocked` accept only `--assignment`/`--refs`, so `--note` on them is refused (report.rs:277). The text itself is the note, capped at 200. `--note` belongs to `report now` only. | WRONG (ambiguous) |
| 98 | `pij anomalies [--here] [--project]` | It also accepts `--seat` (anomalies.rs:245). | MISSING (minor) |
| 14 | `orchestration role set <seat> pm` | Two positional arguments, OK (governance.rs:453). | OK |

The project, stream, fence, dispatch, ack, canary, attest, task (close reasons, governance.rs:1030), report (280/200 limits, model.rs:494,505), decisions (`--asked_by`, decisions.rs:603), answer `--supersede` and spine grammar all match the daemon's parse table (governance.rs:428-455). **~30 OK.**

## prime.md
| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 59 | "broadcast only to … seats it changes; no acks or FYIs" | The FYI held delivery has shipped. protocol.md:119 now reads "no turn-opening FYIs" and endorses `--fyi`, so the route summary is out of date and contradicts it. | STALE |
| 12 | "List `--prime`/`--archived` and tree are unported" | Matches `pij --help`. | OK |

All links and anchors resolve. Link and role-stamping (line 48-50) is correct. **~10 OK.**

## agent.md
No issues. The `pij agent` refusal is confirmed (`pij --help`: "pij agent: E-RS-UNPORTED"). The leaf list (list/show/run/…) is TS-era and can't be checked against rs, but it's harmless. **3 OK.**

## skill.md
| line | claim | reality | verdict |
|---|---|---|---|
| 13 | spawn example | It omits `--role worker` (see delegate). | MISSING |

The canary `--expect-model` and dispatch `--packet --wait` forms are OK (governance.rs:438-440). The `pij agent spawn/report` refusal and the `models` refusal are OK. **~7 OK.**
