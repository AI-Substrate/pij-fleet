# peer — spawn and talk to an rs colleague

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: converse with existing colleagues or stand up a new harness session. [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status) is the stable shim/native ledger: `pij` serves rs or refuses, even with explicit legacy forcing. Do not replay unsupported legacy grammar against another store.

## Identity and views

Follow § C1 delivery-owner detection first. Native OMP, Pi and Copilot extensions each own their runtime's registration/receiving; never re-adopt them or run a competing inbox consumer. Other tmux seats adopt only their exact nonempty current-process pane:

```bash
pij adopt "$TMUX_PANE" --harness <h> ${PIJ_PARENT_ID:+--parent "$PIJ_PARENT_ID"} [--role <asserted-role>] --json
pij whoami --json
pij phonehome --json
pij list --json
pij sessions --json
pij state <seat> --json
pij node show <seat> --json
```

`--json` forwards the complete original v2 envelope; ids live in its real `data`, not a legacy rewrapped object. Shim list forwards exact `--harness <h>`, `--folder <path>`, `--parent <seat>` and `--scope local`. Both shim and native `pij-rs list --here` select the caller's canonical cwd; `--here` is boolean, not a path argument. Sessions has no filters and never unions TS telemetry. Legacy `--role`, `--prime` and `--archived` remain refused. Node show supplies the rs subtree and linked records. RoleService supplies string or null; omitted role preserves existing assertion, explicit unset uses `pij role <seat> --unset`.

For an OMP seat, `pij whoami` reports the loaded OMP extension's `extension_build` and real `extension_path`; for a Pi seat it reports that Pi runtime's loaded extension. Current-daemon null means a pre-144/unreported build (older daemons omit the keys), never infer it from the seat cwd or today's HEAD. Use `pij list --json` for roster build identity.

Parent is the rs descriptor's recorded `parent`, not a spawnedBy fallback. **Roles come from above: pass `--role pm|worker|pa` on the placement call.** On spawn, you become the parent and the role is stamped in the spawn's own transaction. For a hand-started seat (or an omp/pi `pij_spawn` child, pij-fleet#25) run `pij link <seat> --role <r>` after its ready-ping. Link takes a seat with no live parent, or re-roles one you parent. A live foreign parent refuses `E-RS-OWNERSHIP`; a prime or your own ancestor also refuses. Roles are never inferred or backfilled. `tree` still refuses.

Verified external/paneless seats use `pij inbox register --json`, then `pij inbox --wait` from the same native host/session (C1). Native admission derives the matching long-lived host incarnation, never the short-lived CLI's pid; missing/conflicting evidence refuses. Paneless adopt remains unported. Never guess/discover a pane or start a competing extension receiver.

## Spawn and revive: native grammar, not shim fallback

**OMP and Pi are distinct harnesses.** Every spawn requires an explicit `harness` (`omp`, `pi`, `claude`, `copilot` or `codex`); there is no shared default and the parent's runtime does not choose for you. `--bin` is an executable-path override, never a harness selector: `--harness pi --bin omp` refuses; request `--harness omp` instead. A path such as `--harness omp --bin /absolute/path/to/omp` is allowed, but a known different harness basename is refused even in an absolute path. OMP model ids are provider-qualified (`github-copilot/gpt-6-astra`); Pi uses its own installed catalog's exact id (`gpt-5.6-sol` in this example), with provider qualification when needed. A spelling accepted by one runtime does not prove availability in the other.

| Harness | Explicit selection | Native model/effort form |
|---|---|---|
| OMP | `--harness omp`; in-process `harness:"omp"` | `--model github-copilot/gpt-6-astra --effort high` launches OMP with `--model github-copilot/gpt-6-astra --thinking high` |
| Pi | `--harness pi`; in-process `harness:"pi"` | `--model gpt-5.6-sol --effort high` launches Pi with `--model gpt-5.6-sol:high`; use a model from Pi's own catalog |

```bash
pij-rs spawn --harness <omp|pi|claude|copilot|codex> --model <exact-model> [--effort <level>] [--cwd <absolute-path>] [--parent <seat>] [--role <pm|worker|pa>] [--session <tmux-session>] [--name <window-name>] [--allow-retired] --json
pij attest <new-seat> --plan-id <plan-id> --json
pij dispatch <new-seat> --packet <path> --wait --json
pij-rs revive <seat> [--session <tmux-session>] [--name <window-name>] --json
pij-rs revive <seat> --assume-dead --evidence 'Describe why the recorded process binding is stale' --json
```

The shim's `spawn`/`revive` are named refusals because their grammar differs. Native spawn also supports `--id`, `--bin <executable-path>`, `--no-wait`, `--wait-seconds` and Claude inbound-consent flags; it has no legacy `--layout`, `--task`, `--branch` or `--plan-id`. Use its reported pane/window for explicit, owner-controlled tmux placement (§ C5); do not claim a legacy side-stack default. Send a packet after real binding; attest separately. Native revive requires an explicit id; `--print` and `--attach` remain unsupported.

**Daemon-owned retirement policy:** `PIJ_RETIRED_HARNESSES` is a comma-separated daemon configuration, empty by default; generic installations retire no harness. This machine's `just bounce-rs` recipe opts into `PIJ_RETIRED_HARNESSES=pi`. Native and in-process Pi spawn then refuse: `harness pi is retired on this machine; use omp (or pass --allow-retired)`. An intentional native `pij-rs spawn --harness pi --allow-retired …` override is accepted with a spine event; preserve its evidence from `pij spine events --peer <new-seat> --json`. It does not change the machine policy or registration/receiver ownership.

The in-process `pij_spawn` tool reads the daemon's `GET /health` `data.retired_harnesses` through `client.health()` before changing local panes or spawn expectations. It does not duplicate policy from the extension's environment. Use native `--allow-retired` for an explicit override; do not switch executables to bypass refusal. External harness choices use native `/v1/spawn`, not a disguised local Pi launch.

Revive accepts a tombstone **or** `Dead` process observation with the recorded pane absent, without a preliminary close. It records `revive-observed-dead` retirement before relaunch under one spawn lock and cites the tombstone in `details.seq`. `Recycled`/`Unknown` observations require `--assume-dead` plus nonempty `--evidence`: only the recorded parent may override, or a caller with authoritative role `prime` **when the target is parentless**. An unrelated prime has no override on a parented seat. The `revive.assumed-dead` event retains evidence and observation (`details.assumed_dead_seq`); no replacement pid is signalled. `E-RS-REVIVE-LIVE` names the observation and exact override command. An `Active` process or a present recorded pane still vetoes revival, even with the override.

A legacy tombstone row without a matching spine event also revives: `revive.legacy-tombstone` records its stored retirement before relaunch, cited by `details.legacy_tombstone_seq` instead of a fabricated `details.seq`.

An in-process host spawn tool is a separate, explicitly available capability; it does not make an unported CLI flag work. `pij models` catalog enumeration remains unported: agents cannot enumerate per-harness catalogs through pij, and no native catalog command is implied. Discover ids from the selected harness's actual installed catalog. Canary every new/provided/revived peer before trust (§ C2), using runtime footer/round-trip evidence, never a ready-ping alone. Name every owned pane/window, seed § C10 in its packet, and apply § C11 at commit boundaries.

Focus save/list/launch and watch/unwatch are unported; do not silently route them to TS.

## Converse

```bash
pij send <seat> 'literal message text'
pij send <seat> --body-file <path>
pij send <seat> --fyi 'progress note'        # held; opens no turn
pij send <seat> --force --reason 'why' 'text' # wake a cold seat anyway; audited
pij send <seat> --command compact            # compact | new | reload; no body
pij compact-self                             # daemon-resolved self
pij-rs send --to <seat> --command compact     # same control admission
pij inbox --json                             # manual claims; not a competing extension receiver
```

Double-quoted backticks/`$(...)` execute in the sending shell before pij runs. Use single quotes or a literal body file (`-` = stdin) for code/untrusted text. Pointer delivery: persist large content, then send its path. No attachment or broadcast/fan-out substitute is implied by the shim route; unsupported options refuse. Messages follow § C10; let pushed turns wake you rather than polling state.

**FYI, held until the seat's next turn** ([contract](../../../../docs/how/pij-rs-api.md#fyi-held-delivery-plan-158)): `--fyi` (tool form `pij_send({to, message, fyi:true})`; native `pij-rs send --to <seat> --fyi`) stores the message and opens **no** turn: no tmux, UDS or extension push. The receipt says `held (fyi)`. Pending FYIs ride along, appended after the body of the seat's next real pij message or added to its next typed turn by the host hook, each exactly once; until then the seat shows `✉N`. Refused with `--command`, for a remote `seat@machine`, and for a tombstoned seat (a tombstone drops pending FYIs). A body with a `?` is still held, but the receipt warns that it looks like a question. **The FYI rule** (§ C10): Use `--fyi` only when the recipient's next action doesn't depend on it. If they'd be stuck, wrong or waiting without it, it's a normal send. If they'd be fine not reading it until their next turn, it's `--fyi`. If they'd be fine never reading it, don't send it. Always normal sends: work done or a phase complete, review verdicts, hand-offs, blockers, questions, decisions needed. FYI examples: "merged #452, nothing needed from you"; "heads-up: main moved, rebase when you next touch it"; progress notes nobody is waiting on.

**Cold-wake guard** ([contract](../../../../docs/how/pij-rs-api.md#cold-wake-guard-plan-157-phase-2)): the daemon refuses a waking send to a seat that is cold — context over 300k, last API call over an hour ago, not working — with `E-RS-COLD-WAKE` naming the price, and sends nothing. Hold it with `--fyi`, or add `--force --reason '<why>'` (tool form `force:true, reason`; native `pij-rs send --force --reason`) when the wake is worth it; every forced wake is audited as a `send.cold-wake-forced` spine event. FYIs, controls and busy seats are never guarded. Unknown facts allow the send with receipt `cold_check: unknown: <why>`.

**Remote controls on rs** ([API contract](../../../../docs/how/pij-rs-api.md)):
commands carry no body; OMP and Pi each handle claims in-process and acknowledge `executed` or
`refused` + reason. Acceptance is NOT completion. Native extension delivery bypasses typing
grace; Claude PTY keeps it (human drafts hold commands). Copilot refuses with
`E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls`; paneless seats also refuse.
Never replace refusal with literal `/compact` or send-keys. Any registered peer may compact,
without arming. Destructive `new`/`reload` require self or target-recorded parent, resolved
from daemon caller evidence — prime flags/roles confer no authority (no rs target-prime relation).
In either OMP or Pi, the target's human must run `/pij` to arm `new`/`reload`; unarmed requests refuse, never defer: arm and resend. Pi reload needs re-arming; installed OMP reload reopens the native session, NOT extension/skill resources.
Expired claims terminate with unknown-outcome/missing-ack, never replay; refused ids stay refused. Intentional retry after arming requires a new message id.

`compact-self` takes no legacy instruction/pane override; it remains a shipped structured daemon operation. `pij bg create/list/tail/kill` remains shipped unchanged in semantics (§ C7): detached runner, completion injected from `pij-bg`, bounded output and owner/recorded-parent recovery. Neither introduces answer queue withdrawal.

**Body recovery in OMP and in Pi** ([wire contract](../../../../docs/how/pij-rs-api.md#extension-inbox-recovery)):
unconsumed delivery idle for `PIJ_REDELIVER_IDLE_MS` (default 10s) resends through
the prompt path, marked `[pij resend n]`; lease reclaim only refreshes tracking.
The first observed agent boundary grants one prompt resend after `PIJ_BOUNDARY_GRACE_MS`
(default 2s) even while busy; native consumption cancels it and later retries retain
the idle rule. Compaction defers delivery until `session_compact`, `turn_start`,
`agent_end`, `message_start`, or the `PIJ_COMPACTION_LATCH_MAX_MS` ceiling (default 120s);
release re-polls for missed pushes. Every 20s and before resend the extension heartbeats
(`seat`, `job_id`): running claims renew without ACK; validated matching-job
`done`/`failed` clears tracking without reinjection. Unknown/legacy/error responses
preserve idle recovery with a 20s probe throttle; stream outages defer transport
work to the existing reconnect backoff, never suppressing local idle resends.
Live `working` seats also renew at expiry (a one-directional safety interlock).
Three resends without consumption park as `undelivered:harness-swallowed`; three silent,
unprotected expired leases (default 60s, `PIJ_RS_EXT_CLAIM_LEASE_SECS`) park as
`undelivered:lease-exhausted`. Both emit sender-visible failure and `delivery.parked`.
Controls retain their non-replay contract above.
`pij-rs inbox --seat <seat> --peek --json` distinguishes parked rows.
`pij-rs inbox release --seat <seat> --job <id> --evidence '<observed blockage>'`
parks the running body head as `undelivered:operator-released`; only its recorded
parent or authoritative prime may do this. It neither sends keys nor kills a seat.
The pending badge excludes
recent resends and clears parked ids with a visible notice; pending forever is
delivery failure, not successful queuing.

## Transcript versus events

`pij tail <seat>` refuses `E-RS-UNPORTED`: native `pij-rs tail` follows **daemon event frames**, not a peer transcript. Never substitute one for the other. For runtime/footer evidence use the actual host's supported transcript or an owner-scoped tmux capture, not an invented rs transcript API.

```bash
pij-rs tail --since <machine>=<cursor>    # event replay, not a transcript
```

The API is NDJSON: Hello `{build,hello:true,v:1}`, not `type=hello`; Event payload is a JSON **string**. Replay uses per-machine exclusive cursor maps over HTTP; the native CLI uses repeated `--since MACHINE=CURSOR`. See the stable API and canonical event fixture before decoding.

## Retirement and failure boundaries

```bash
pij close <seat> [--reason <text>] --json
pij reap --dry-run --json
```

Close requires self or the current recorded parent. It tombstones the seat and preserves history, **not pane/process teardown**; no `--force`. Healthy peers should be compacted/reused. A separately authorized owner handles terminal teardown after actual liveness evidence; reap is conservative stale-record repair (ops route), not a killer.

The daemon's **death sweep** runs every **5s** by default (`PIJ_RS_DEATH_SWEEP_MS`, milliseconds). It applies the same conservative rule as `pij-rs reap --dry-run`: `Dead`/`Recycled` process plus recorded pane absent; unknown evidence is not death. It writes one `seat.tombstone` with `reason: "observed-dead"` and the process/pane observation, then **pushes an obituary to the recorded parent** through the existing delivery service. The sender is the same daemon-owned `pij-bg` virtual actor used for bg completion (`pij_core::BG_ACTOR`), not a second identity. Do not poll for child death.

A tombstoned parent or one observed-dead in the same sweep receives no notice; the daemon logs the withheld count instead, avoiding reboot floods. Queued delivery does not prove parent observation. The obituary names the supported native `pij-rs revive <id>` command. See [Death sweep](../../../../docs/how/pij-rs-api.md#death-sweep) for the exact template and [Revive](../../../../docs/how/pij-rs-api.md#revive) for the override boundary.

| Observation | Move |
|---|---|
| Self cannot resolve | Respect extension ownership; non-extension tmux seats may adopt only their exact current pane |
| Tombstoned self still has a live pid | Identity must refuse; inspect tombstone evidence, not a legacy descriptor |
| Unsupported flag/leaf or legacy force | Read the stable refusal ledger; no hidden fallback |
| Reboot / stale pane or pid | Re-observe; native revive accepts a tombstone or `Dead` + pane absent; `Recycled`/`Unknown` needs the authorized evidence-bearing override |
| A new peer is ready but its first inference fails | Inspect actual model/footer/runtime; canary before dispatch |
| Send acceptance without execution | Inspect the actual receipt/control outcome; acceptance is not completion |

Examples here are operating grammar, not executed smoke receipts. Canonical API/event examples are in `crates/testkit/fixtures/golden/api/`; PM/prime own runtime and production/chainglass proof.
