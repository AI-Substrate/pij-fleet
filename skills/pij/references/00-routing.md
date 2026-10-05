# /pij · routing & shared conventions

Loaded by the dispatch (`../SKILL.md`) in **guided mode**, together with the chosen route module only. Direct jumps don't load this up front — a route module may cite a `§ Shared conventions` block below and pull it lazily; that is still progressive disclosure.

**rs serve-or-refuse is the machine contract.** Read [Unsupported status](../../../docs/how/pij-rs-api.md#unsupported-status) for named shim/native gaps. Explicit `PIJ_DAEMON_GENERATION=legacy`, missing credentials and daemon absence never enable fallback. Raw `--json` preserves the complete original v2 envelope. Shim list forwards exact `--harness`, `--folder`, `--parent`, `--scope local` query values; shim and native `pij-rs list --here` scope to caller cwd. Sessions has no filters or legacy union. Canonical fixtures live in `crates/testkit/fixtures/golden/api/`, not new live captures or production acceptance.

## Detection signals (guided `/pij`, re-derived every call — survives /compact)

All probes are deterministic files/commands; remember nothing between calls.

| # | Signal | Exact probe | Route offered |
|---|--------|-------------|---------------|
| A | Open delegation run | newest `.flow-pair/runs/*/run.json` has `"status": "open"` (schema enum is `open\|closed` — `skills/flow-pair/schemas/run.schema.json`; missing file/status = no signal) | **offer** resume `pair` — never auto-resume; stale-open runs are common |
| B | Live fleet roster | newest run roster ids, corroborated through `pij state <id> --json`; legacy descriptor files are not rs liveness evidence | `pair` reattach or `ops` inspect owned retirement |
| C | Active the-flow mid-build | newest `docs/plans/*/the-flow.json` → `harness flow nav show --path <it>`; `data.nav.now`/`next` on a phase/review node | offer `pair` dispatch for that phase |
| D | Daemon alive | `pij-rs ping --json` | unavailable + spawn intent → operator-owned `ops` readiness; no automatic legacy daemon start |
| E | Delivery owner | Copilot's native `pij_send` callable → native Copilot; else in-process `pij_spawn` callable → extension push (distinct OMP and Pi modes in C1); else exact non-empty `$TMUX_PANE` → tmux push; otherwise external pull where supported | detect before self-registration advice; native Copilot never falls back to CLI pull or sendkeys |
| F | Self registration | after E: extensions own registration; non-extension tmux peers may probe `pij whoami`; paneless claims require verified native admission | verify native evidence before external inbox registration; never re-adopt an extension-owned seat |

**Precedence**: A > B > C > D/F (D/F are preconditions folded into whichever route wins, not destinations). Nothing matches → ask the job in one line, listing the registry.

**A hint is never a command**: `/pij pair` with no daemon → boot first; `/pij pair` with no open run and no active flow → confirm intent to start fresh. Validate the precondition, redirect with one line of why.

## Shared conventions

Cited by route modules as "§ C*n*" — prose lives here only.

### C1 — Harness and delivery modes

Delivery-owner detection happens before any self-registration advice. A Copilot session exposing the native `pij_send` tool is **native-extension-owned**: registration and receipt injection are automatic, so do not adopt it or start a competing CLI inbox consumer. Its `pij_send` takes `fyi:true` for a held FYI, as in OMP and Pi. Missing native capability queues/refuses, never sendkeys. Copilot programmatic compact/new/reload are unsupported; use native human lifecycle controls, not typed-message fallbacks. Its consent sensor still requires a supported pane unless the operator explicitly disables typing grace.

For the remaining modes, detect once per run: callable in-process `pij_spawn` → **extension push**, using the current host's OMP or Pi receiver; otherwise exact non-empty `$TMUX_PANE` → **tmux control-plane push**; otherwise **external pull** where supported. Receiver ownership does not select a child's harness: every spawn names it explicitly, with no shared default. The table below covers these remaining modes, not native Copilot ownership.

| Intent | OMP push | Pi push | tmux control-plane push | external pull |
|---|---|---|---|---|
| prereq | **OMP extension owns registration: no adopt/inbox register.** | **Pi extension owns registration: no adopt/inbox register.** | running rs daemon + exact-pane adoption: `pij adopt "$TMUX_PANE" --harness <h> ${PIJ_PARENT_ID:+--parent "$PIJ_PARENT_ID"}` | `pij inbox register --json` from a verified Claude/Copilot/Codex tool shell; accepted id is `data.id` |
| spawn | `pij_spawn({harness:"omp", …})` for an OMP child; another child needs its own explicit harness | `pij_spawn({harness:"pi", …})` for a Pi child, subject to daemon retirement policy | `pij-rs spawn --harness <h> --cwd <absolute-path> --parent <id> …` | no tmux launch without an available tmux context |
| message | `pij_send({to, message})` | `pij_send({to, message})` | `pij send <id> '<text>'` | `pij send <id> '<text>' --json` from the same native host/session |
| FYI (next action doesn't depend on it; held, opens no turn) | `pij_send({to, message, fyi:true})` | `pij_send({to, message, fyi:true})` | `pij send <id> --fyi '<text>'` | `pij send <id> --fyi '<text>' --json` |
| receive | automatic OMP in-process injected turn | automatic Pi in-process injected turn | automatic daemon-injected turn | `pij inbox --wait [ms] [--json]`; never a competing extension receiver |
| compact peer | `pij_send({to, command:"compact"})` | `pij_send({to, command:"compact"})` | `pij send <id> --command compact` | same structured command, subject to target support |
| peek | OMP transcript or owner-scoped tmux capture | Pi transcript or owner-scoped tmux capture | host transcript or owner-scoped tmux capture | supported host transcript only; shim tail refuses |
| retire | owner-authorized OMP teardown is distinct from rs tombstone | owner-authorized Pi teardown is distinct from rs tombstone | `pij close <id>` tombstones only, self/current-parent | same rs ownership, no force bypass |

Tmux self-adopt uses only the exact nonempty `$TMUX_PANE` of the current process. Carry the supported `--parent` claim when the environment names the true governor; omitted parent is unsaid and preserves an existing value. Existing-seat structural repair through `link` is unported, so never omit parent assuming that command can fix it later. Native extensions retain ownership of their own registration.

Empty or absent `TMUX_PANE` is not permission to discover or guess another pane.
Paneless adopt still refuses; **`pij inbox register --json` is the supported
external pull bridge to native `/v1/register`**, not a generic shim `register`
verb or a legacy store. Prerequisites are the current `CLAUDE_CODE_SESSION_ID`,
or a UUID `COPILOT_AGENT_SESSION_ID` with its matching `~/.copilot/session-state/<id>`
directory, or a UUID `CODEX_THREAD_ID` with its matching readable date-nested
rollout under `~/.codex/sessions`. Multiple valid identities refuse.

The CLI must descend from the matching live harness host. rs observes process
ancestry and start stamps, rejects a nearer conflicting harness/session, and
binds the **host** incarnation rather than the transient CLI/shell. Repeated
registration of the same host/session returns the same `data.id`; an inherited
or stale `PIJ_SESSION_ID` is replaced only after that native admission succeeds.
Subsequent shim whoami/phonehome/send/inbox resolve the same native identity
without a manual export. Missing evidence is a named refusal, never permission
to fabricate a process or fall back to FsRegistry/FsChannel.

Pull seats have `pane:null` and `native_extension_delivery:false`. `inbox --wait`
blocks for mail; a positive millisecond argument bounds the wait. JSON remains a
complete v2 envelope whose `data` is a claims array (empty on finite timeout),
not an unwrapped `messages` object. The CLI prints before acknowledging; claim
acceptance is not model completion. Native-extension-owned OMP, Pi and Copilot seats
keep their automatic receiver and must not start this pull consumer.

Model names and permission/launch behavior belong to the selected native harness and installed rs source, not legacy spawn defaults.

### C2 — Canary-verify (a ready-ping is NOT proof)

A ready-ping is not proof of a working model. Run `pij canary <id> --expect-model <m>` after spawn and before trusting a provided peer: nonce-correlated dispatch/ack and observed process/session/model evidence are required. Capture the actual host footer or owner-scoped tmux pane when a model cannot be corroborated; `pij tail` is unported and native tail is events, not a transcript. Do not mark an unpinned or unacknowledged peer healthy.

### C3 — Compact discipline (early, not late)

The instant a reusable/live coder reports completion or a reviewer returns a verdict, send compact as the **first tool action** — before reading, synthesising, or acting on the report. Trigger only on a terminal completion/verdict, never while the peer is still responding. The 30–90s compact latency overlaps report/review/fix work that must happen anyway; compacting late has caused post-dispatch stalls.

Dispatch compact **fire-and-forget** with `pij_send({to, command:"compact"})` or `pij send <id> --command compact`, without `--wait`. **Continue immediately** with independent report/review work; never wait for compact receipts or execution. Acceptance, receipt and executed/refused outcome remain distinct. Copilot/paneless controls refuse; never replace a refusal with literal slash text or sendkeys.

The former one-shot `--once` state flag is unported; use the already one-shot `pij state <seat> --json` snapshot ([Unsupported status](../../../docs/how/pij-rs-api.md#unsupported-status)). State snapshots and a host's `E-DEAD` refusal are observe-only diagnostics, never progress gates. Do not poll for recovery or gate report/review work on a compact acknowledgement.

Between phases: compact, keep, reuse — never close-and-respawn a healthy peer. `pij compact-self` is the shipped daemon-resolved operation, with no legacy instruction argument or `--pane` override. Remote new/reload retain target arming and self/recorded-parent authorization; role/prime display grants none. See peer route for the unchanged complete control contract.

### C4 — Model discovery

`pij models` is unported and returns `E-RS-UNPORTED`: agents still cannot enumerate per-harness catalogs through pij, and there is no native catalog substitute here. Use the selected harness's actual installed model catalog, then corroborate the selection with canary/runtime evidence (C2). OMP uses provider-qualified ids; Pi uses its own catalog's exact model id, with provider qualification when needed. The [peer spawn contract](./routes/peer.md#spawn-and-revive-native-grammar-not-shim-fallback) gives separate examples, executable-override rules and daemon retirement policy. Do not substitute a stale legacy model list or invent a selector.

### C5 — Placement & split-cap

Native `pij-rs spawn` accepts `--session`, `--name`, `--cwd` and `--parent`, not the legacy `--layout stack|right|below|window` flags. Use explicitly supported host spawn tools where available, or place the native reported pane using owner-controlled tmux operations. A tool's side-stack/layout support is not a claim about native CLI grammar. Keep new peers visible and grouped by stream; no invented split-cap or headless contract.

**MANDATE — name every pane and window you create.** A tmux surface is a *human*
interface: the operator scans it to see what the fleet is doing, and a wall of
identical `pi-peer` windows is unreadable — they cannot tell which seat to look at,
interrupt, or reap. Spawning is not finished until the seat is labelled. Immediately
after any spawn:

```
tmux rename-window -t <window> "<stream>-<job>"          # e.g. s066-revive
tmux select-pane   -t <pane>   -T "<stream> <job> · <peer> · <model>"
```

Rules: name by **what the seat is doing**, not what it is — `s066-revive`, not
`pi-peer`/`worker-3`. Include the stream/plan id when there is one, so a window maps
to a branch and a brief. Keep it short enough to read in a status bar. Re-label if the
seat's job changes. This applies to every layout (`window` names the window; `stack`/
`right`/`below` name the pane title) and to peers you adopt as well as ones you spawn.
The operator should never have to ask "what is that pane?" — if they do, the mandate
was skipped.

**MANDATE — one window per stream: keep a team together, don't sprawl.** A coder and its
reviewer belong to the same piece of work, so they belong in the same tmux **window**, as
side-by-side panes. One window per stream/plan id, named for that stream — not one window
per peer. Sprawl is the failure mode: a fleet spread over a dozen identically-named windows
is unreadable, and the operator loses the ability to glance at a stream and see its whole
state at once.

#### The standard team window (DEFAULT — build every project team this way)

A project team is **PM + coder + reviewer**. Its window has one canonical shape: the **PM
owns the left half**, and the **coder and reviewer stack in the right half**, coder on top.

```
┌──────────────────────┬──────────────────────┐
│                      │  coder               │
│  PM / orchestrator   ├──────────────────────┤
│  (left 50%)          │  reviewer            │
└──────────────────────┴──────────────────────┘
```

Why this shape: the PM is the pane a human talks to, so it gets the stable half and stays put
as workers come and go. Coder and reviewer are a *pair* — stacking them puts the work and its
critique adjacent, and a two-round review reads top-to-bottom. The window is named for the
stream, so one glance answers "what is this team doing, and where is it up to".

Build it in this order — PM first, because it anchors the layout:

```
# 1. PM takes the window; split off the right half
tmux split-window -h -t <pm-pane> -p 50

# 2. coder joins the right half, reviewer stacks BELOW it (spawn with --layout window, then move)
tmux join-pane -v -s <coder-pane>    -t <right-pane>
tmux join-pane -v -s <reviewer-pane> -t <coder-pane>

# 3. even the right column, then title every pane (naming mandate above)
tmux select-layout -t <coder-pane> even-vertical
```

When native spawn creates a window, move its reported pane into the intended owned team window and title it. Spawning is not finished until placement and identity are verified; do not pass an unported layout flag to the shim.

**When the PM is a central orchestrator** driving several streams at once, it cannot sit in
every team window. Then the left half holds that stream's operator view — a shell in the
stream's worktree or its host's supported transcript view — and the slot is reserved so a per-stream orchestrator can take it later without re-laying-out the window. Native rs tail is not a peer transcript.

Split a team across windows only when panes get too small to read — and say so when you do.

### C6 — Daemon restart rule

The rs daemon executes a built Rust binary, not tsx source. Changes need the appropriate rebuild and an operator-authorized native lifecycle action; never bounce production as an incidental edit step. `pij daemon` is a named shim refusal. Check the installed `pij-rs daemon --help`; PM/prime own composed validation and production restart. Peer extension/skill reload is a separate host capability, not daemon restart or the remote OMP reload control.

### C7 — Push when owned; block on inbox when pull-owned

In pi/tmux push modes (including OMP and native Copilot), injected turns re-invoke you. After dispatch, do independent prep and let delivery wake you; never poll `pij state`. In verified external pull mode, register through C1 and use `pij inbox --wait` (native `pij-rs inbox --wait` is also available). This blocking inbox read is the delivery primitive, not a competing extension receiver or a state poll. A broken transport justifies bounded host-transcript/pane inspection, not substituting native event tail for a transcript.

**The same rule applies to SLOW LOCAL COMMANDS — use `pij bg`.** A command you sit and wait on holds your turn open for its whole duration; you are idle-but-not-done, which is the shape the watchdog derives as a stall, and the human cannot talk to you meanwhile. For rs-hosted seats, `pij bg` and `pij-rs bg` both ask the Rust daemon to run it detached and deliver the result as an injected turn from `pij-bg`, so your turn ends now and the completion is what wakes you. Both spellings return the same complete v2 envelope with `--json` before or after the leaf.

```bash
pij bg create --title "harness checks" --command "harness checks" [--cwd <dir>] [--timeout 30m]
pij bg list [--all]          # your jobs with running time / duration; --all adds children's jobs
pij bg tail <job> [--lines N]  # bounded snapshot of a job's output
pij bg kill <job>            # stop it — and still get a turn back
```

It returns immediately with a job id; later a turn arrives:

```
[pij bg] OK — harness checks (5m12s) · full log: <daemon-state-dir>/bg/<job>.log · tail: …
```

Reach for it whenever a command runs longer than a few seconds — `harness checks`, builds, full test runs, `gh run watch`, long clones. **The `--command` string is passed literally to `/bin/sh`** in your current directory (`--cwd` overrides; relative paths resolve against your cwd), so pipe, redirect, and chain freely — shape the output to what you actually want back, because only the last 20 lines ride inline. The daemon owns job state and the full stdout/stderr log at `<daemon-state-dir>/bg/<job>.log` (mode `0600`); `tail` reads it server-side. Title it for the reader: it is how a human knows what just fired back.

`list`/`tail`/`kill` are RECOVERY, not routine — the completion turn stays the primary signal. `list` includes your finished jobs without `--all`; `--all` additionally includes only directly recorded children's jobs, never every seat's jobs. Only the owner or its recorded parent may tail or kill a job. `tail` is deliberately a bounded snapshot with no `--follow`: a follow loop would quietly reinstate the blocking wait `bg` exists to remove. `--timeout` kills an overrunning job and delivers `[pij bg] TIMEOUT — <title> (killed after <limit>)` with the log tail. `kill` still delivers a turn (`[pij bg] KILLED — <title>`), because a silent kill leaves you waiting forever for a result that can never arrive. On restart, the daemon re-adopts surviving jobs by pid **and process-start identity** and recovers durable completions; if neither can be recovered it marks the job `lost` and sends a LOST turn naming the log. Restart does not change the owner, and a recycled pid is never enough to re-adopt.

**Event sources — one program, many turns.** `pij bg create --events [--fyi] [--min-interval 60s] [--inline-max 5] --title T --command …` runs a long-lived program that fires events back to you until it exits or you `pij bg kill` it. The child gets `PIJ_BG_JOB` and a per-job secret `PIJ_BG_TOKEN` (not the daemon key; it dies at kill or exit) and fires with `pij bg emit [--data <json>|--data-file <path>] "<text>"` (or `POST /v1/bg/{job}/emit` with `Authorization: Bearer $PIJ_BG_TOKEN`). Events that arrive while you are busy, or within `--min-interval` of the last wake, arrive together as one turn: up to `--inline-max` listed inline, more as `[pij bg] N new events from <title>, here is the file: <path>`. `--fyi` holds each batch for your next turn instead of waking you. A cold owner (the cold-wake guard's check) is never woken: the batch is held as an FYI and its prime (or, if none is warm, the human by Telegram) is told and decides. The end arrives as a final turn (`STOPPED`, or `OK/FAILED (exit N)`) with the events since the last batch, delivered by the same rules: held with `--fyi`, and for a cold owner held and escalated, never woken. `list` shows events fired, pending and the last fire. At most 10,000 events wait per source; beyond that they are dropped and the next batch says how many.

```bash
pij bg create --events --title db-watch --command '
  last=$(psql -Atc "select max(id) from orders")
  while sleep 30; do
    now=$(psql -Atc "select max(id) from orders")
    if [ "$now" != "$last" ]; then
      pij bg emit --data "{\"from\":$last,\"to\":$now}" "new orders $last→$now"
      last=$now
    fi
  done'
```

Two things it is NOT: not a way to message yourself (`pij send <self>` is still E-SELF — bg delivers as the `pij-bg` actor because the result genuinely comes from the runner), and not a substitute for peer delegation. One command whose output you want → `pij bg`. A unit of work needing judgement → a peer.

### C8 — Terminal and no-show interpretation

Do not call an absent peer a crash from pane/PID absence alone. A persisted
pij-owned close is **requested**; an observed absence without that intent is
**unrequested-by-pij**; a failed probe is **unavailable**. Daemon reports label
its initial boot reconciliation **historical** and later evidence **live**.

Each launch has a bounded expectation keyed by `spawnId`; expiry means only that
that expected registration did not appear. If a descriptor with the same key is
present, it suppresses the no-show. Never substitute a guessed harness, cause, or
owner for either observation.

### C9 — Watchdog etiquette (you may be watched; some peers must never be)

The `pij watchdog` administrative family is unported and returns `E-RS-UNPORTED`; do not execute its former watch/pause/exempt/reset/interval controls or manipulate legacy sidecars. This does not remove already-shipped daemon mechanisms. Do not claim the old universal 20-minute setup, pause tiers or PA subscription grammar as rs contracts.

If a real nudge arrives, report actual now/next or the truthful question/blocked state and continue independent work. If done, run `pij report state done`. Reporting at both work edges remains the primary cadence; a nudge is only a backstop. Use shipped `pij bg` (C7) for slow commands rather than keeping a coordinating seat blocked. An unavailable PA watchdog subscription is a named operational gap, never a fabricated supervision graph.

### C10 — Wire discipline (A2A messages)

**Canonical copy — cite "C10" from other modules; never restate these rules.** Governs every message from one agent to another (`pij send`, inbox replies, pushed reports). Human-facing prose is out of scope. The recipient is a machine — write for a machine reader. Evidence: plan 083 (fleet measured ~2.9M tokens of A2A bodies; the waste is acks, restatement, and praise — not long analysis).

1. **Line 1 is the recipient's next ACTION or DECISION — or `NO ACTION`.** The reader may stop there; everything below line 1 is optional context.
2. **Delta only, with the discriminating value.** State what changed + the one count/SHA/path that could have been wrong. Cite rulings and prior messages by id — never restate them, never restate the recipient's own words back to them, never itemize unchanged state (one denominator line max).
3. **Don't send:** unsolicited confirmations — silence after a clean verify **is** the all-clear; a *requested* check returns one line (`checked X, clear`). Praise never travels as its own message — attach it to an instruction or drop it. Use `--fyi` only when the recipient's next action doesn't depend on it; questions, blockers, hand-offs, verdicts and work done are normal sends, and what they'd be fine never reading isn't sent ([the FYI rule](routes/peer.md#converse)).
4. **Acks are one line** and never restate the instruction.
5. **Exception — reasoning IS the payload** when correcting a false belief, disagreeing, or acting on low confidence with high impact: send the full why and flag the trigger (`correction:` / `dissent:` / `confidence: low`) so the receiver knows to read past line 1. Rare; never use it as cover.
6. **Telegraphic is fine; ambiguous is not.** Keep every identifier, number, path, and scope marker intact. Terse *common* words beat invented shorthand — tokenizers punish rare strings, and private codes fail silently across models. Do not mirror a verbose peer's style.
7. **Never reconcile a contradiction.** A receipt or instrument output that contradicts itself is a FINDING, not a formatting problem — relay it verbatim, contradictions intact, with your summary ABOVE the raw output, never instead of it — and the summary line itself must NAME the contradiction (a clean-sounding line 1 over buried evidence re-creates the failure this rule exists to stop). Terseness compresses YOUR words; it never smooths EVIDENCE (a tidied "no notable changes" once nearly destroyed the only trace of a live fleet defect — plan 083).

Pre-send check, two questions: could the receiver act on line 1 alone? Did I restate anything they already know?

### C11 — Commit attribution

Every commit created by a pij seat carries attribution **at commit time**. Run `pij commit-trailers` (no arguments) in the committing worktree immediately before committing. This is narrow forwarding to native `pij-rs commit-trailers`: trailer-only stdout, native stderr derivation-tier note and exit status are preserved; no HTTP or JSON wrapper. Append emitted `Key: value` lines after a blank line alongside, never replacing, the harness's own trailers. Values are derived, never hand-typed:

| Key | Derived value / omission |
|---|---|
| `Pij-Seat` | Committing seat from `pij-rs whoami`; omitted when the derived `Pij-Prime` is that same seat. |
| `Pij-Prime` | Nearest explicit `role == prime` on rs self/ancestor rows, else the complete recorded-parent root (explicit interim fallback). The native helper currently supplies no repository-designation lookup; do not claim that tier was consulted. Its stderr names the tier used. |
| `Pij-Plan` | Worktree path, branch, then local flow context. The helper does not currently query current assignment; missing evidence is omission, not a guessed plan. |

The helper prints only applicable trailers to stdout and unresolved-value diagnostics to stderr. If an expected value cannot be derived, omit that trailer and explain the omission in the commit body; never guess or emit a placeholder. Current context is not evidence of a historical commit's identity.

### C12 — Shared machine: heavy work runs niced and capped

The fleet shares Jordan's working laptop. Builds, test loops, stress and flake reproductions, browsers and benchmarks run under `nice -n 19` with explicit caps:
- at most 8 parallel workers (`xargs -P 8`, `--test-threads 8`);
- `cargo -j 4`;
- a wall-clock bound (`timeout 600`);
- stop when the load average passes 40.

A packet asking for repeated or "under load" runs states these caps. An uncapped `-P 48` flake reproduction once took the machine to load ~230 and had to be killed (2026-09-29). Reproduce a timing flake by reasoning about what decides the race, or with a deterministic hook, before brute force.
