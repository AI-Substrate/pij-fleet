# ops — Rust daemon, binding and record hygiene

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: inspect the rs control plane and reconcile stale records without treating uncertainty as death. [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status) distinguishes the shim from native CLI. `pij daemon`, `path` and `telegram` refuse `E-RS-UNPORTED`; there is no legacy-force escape.

## Daemon

```bash
pij-rs ping --json
pij-rs doctor claude-inbound|claude-hook|claude-statusline --json   # read-only Claude-home audit
pij-rs daemon --bind <host:port>           # foreground service; use an owned supervisor
pij-rs daemon bounce                       # rebuild, drain, restart, verify health (authorized only)
```

`just bounce-rs` is this machine's production bounce (it also sets `PIJ_RETIRED_HARNESSES=pi` and the Telegram env); `pij-rs peers check` audits [federation](../../../../docs/how/pij-federation.md) pairings. Do not substitute the former `pij daemon start/stop/kill` commands or assume spawn auto-starts a daemon. Rust changes require a rebuilt binary and an **authorized** restart (§ C6), not tsx hot-reload. Production rebuild/bounce and acceptance are prime-owned. An unavailable daemon or bearer credential is a real error, never permission to inspect/mutate a legacy store.

`PIJ_RETIRED_HARNESSES` is daemon-owned, comma-separated configuration with an empty generic default. Only this machine's `just bounce-rs` recipe opts into `pi`; do not treat that as a global Pi ban or set extension-local policy. `GET /health` exposes `data.retired_harnesses` for in-process spawn's pre-mutation policy check. Native `pij-rs spawn --harness pi --allow-retired …` is the explicit audited override, not an executable switch. See the [spawn contract](../../../../docs/how/pij-rs-api.md#spawn-harness-choice-and-retirement-policy); this guidance does not authorize a production bounce.

Every fixture daemon **and client** must use the same private port, state directory, HOME, CLAUDE_CONFIG_DIR, XDG_CONFIG_HOME and private tmux socket. Canonical isolation recipe: `crates/testkit/fixtures/golden/api/governance-routes.json#isolation`. Never use production port 7461 or a shared tmux tap for a fixture. Canonical examples are not new runtime captures.

## Read the rs store, not legacy descriptors

```bash
pij list --json                    # active rs seats; complete v2 envelope
pij sessions --json                # rs-only session rows, no legacy union
pij state <seat> --json
pij node show <seat> --json
pij phonehome --json               # from that seat's own context
pij anomalies --json
```

Shim `pij list` forwards exact `--harness <h>`, `--folder <path>`, `--parent <seat>` and `--scope local` to `/v1/seats`. Both shim and native `pij-rs list --here` select the caller's canonical cwd; `--here` is boolean, not a path argument. Sessions remains bare/`--json` only. Legacy `--role`, `--prime` and `--archived` filters refuse rather than disappear. `~/.pij/<id>.json`, archive directories and filesystem ledgers are **not rs authority**. Absence from the active roster does not prove a seat never existed: inspect tombstone evidence through supported rs views. Never restore, delete or patch legacy descriptors to repair rs identity.

Phonehome re-observes process identity; `bound` is not merely presence of a pid. The optional role is joined through RoleService like whoami/seats/StateCard: string assertion or null, never an inferred default. A tombstoned seat cannot authenticate as a live identity even if its old process still exists.

## Close versus reaping

```bash
pij close <seat> --reason 'owner-approved retirement' --json
pij reap --dry-run --json
# After authorized review of the candidates and unverifiable rows:
pij reap --json
```

Close requires self or the target's current recorded parent. It atomically records a tombstone and event; it **does not kill a process or pane**. There is no `--force` escape. A parent repeat-close returns the existing tombstone; retired self still receives an identity refusal with that evidence. Keep healthy peers across work items; compact/reuse instead of retirement (§ C3).

Reap is stale-record reconciliation, not process termination or age-based retention. A candidate needs confirmed dead/recycled process identity **and** confirmed absence of its recorded pane. Paneless seats need confirmed dead/recycled process identity alone. A pane id that exists again but hosts a later process counts as absent (**reincarnated**, the usual post-reboot case). Live process/pane evidence retains the seat; unknown process/pane evidence is `unverifiable` and brakes retirement. Removing an unknown-evidence check can only permit the same set or more reaps: a **brake**, not product policy. Dry-run changes no store/spine rows. Execution re-observes the binding before conditional atomic retirement, protecting a concurrently revived incarnation. Never signal a recycled pid.

**Exact ps absence protocol**: `LC_ALL=C ps -o lstart=PIJ_LSTART -p <pid>` returns exit 1, empty stderr, and one UTF-8 header line trimming to `PIJ_LSTART`, with no subsequent line. Empty stdout is not absence. Exit 0 needs header + exactly one parseable process row. Extra blank/data rows, any stderr (including whitespace), wrong header, other exits, signals and parse failures are unknown. Source and named existing tests: [`crates/harnesses/src/proc.rs`](../../../../crates/harnesses/src/proc.rs), especially `exit_one_with_header_only_and_empty_stderr_is_absent` and `failed_probe_requires_exact_header_only_output`. These names are not a claim of a new validation run.

## Recover after a reboot

Re-derive every process/pane observation; old numbers belong to the previous boot.

1. `pij-rs ping --json`, then `pij list --json` and `pij state <seat> --json`. On restart the death sweep (§ C8) tombstones seats whose process is dead or recycled and whose pane is absent or reincarnated (`observed-dead`); obituaries to parents that died too are withheld, so read tombstones rather than wait for notices.
2. **Check nothing already runs the session before reviving.** Take `data.session` from `pij state <seat> --json`, then `ps -axo pid,lstart,command | grep <session>` and `tmux list-panes -a -F '#{pane_id} #{pane_pid} #{pane_current_command}'`. Revive never checks a tombstoned seat's own process or pane, and sees a non-Claude copy only if it is registered, so a still-running or hand-resumed copy would become a second seat (pij-fleet#55, open). If a copy runs, adopt that pane instead (`pij-rs adopt <pane> --harness <h>`, or `--reclaim <seat>` as its parent or prime).
3. `pij-rs revive <seat> [--session <tmux-session>] [--name <window-name>] --json` resumes the recorded conversation (refusals and `--fresh`: peer route § Spawn and revive).
4. Canary before trust (§ C2); revive does not wait for registration.

A refusal is not permission to copy an archived TS descriptor into rs, guess a prime, or start a fresh session wearing an old identity. Folder-based prime revival remains a named gap; human choice of successor/launch is still required where ownership or identity cannot be established.

## Tmux and control hygiene

Retiring a record and closing an owned terminal are separate actions. Match live rs pane/process evidence to the actual terminal before any separately authorized teardown. Never kill an unowned pane or turn a failed probe into absence.

Shipped controls, compact-self and `pij bg` remain available; this cutover does not remove or reimplement them. Remote controls use structured command admission, target arming and acknowledged outcomes, never literal slash/sendkeys fallback. bg owns detached process groups, durable completion injection and bounded log reads; its kill operation is not answer queue withdrawal. See § C3/C7 and the peer route.

## Diagnosis

| Observation | Action |
|---|---|
| Native ping/auth fails | Repair the intended rs address/credential or escalate to its operator; never fall back |
| Phonehome cannot resolve self | Respect extension-owned registration; otherwise adopt only the exact observed current pane (§ C1) |
| Active roster lacks an id | Inspect rs history/node evidence; no legacy file restoration |
| Process/pane probe is unknown | Retain the record and report the unverifiable evidence |
| Pending answer cannot supersede | Wait for a recorded terminal outcome; no queue withdrawal |
| Government files unavailable | Resolve per bootstrap § Governance branch; if none exists, say so and use store-native records; never substitute main |

Government documents live at the worktree `harness/scripts/government-root.sh` resolves, never a product/stream write target; pij-fleet has none today (the resolver exits 1), see bootstrap § Governance branch. Production and chainglass confirmation remain prime-owned.
