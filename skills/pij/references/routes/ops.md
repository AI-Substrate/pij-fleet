# ops — Rust daemon, binding and record hygiene

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: inspect the rs control plane and reconcile stale records without treating uncertainty as death. [Unsupported status](../../../../docs/how/pij-rs-api.md#unsupported-status) distinguishes the shim from native CLI. `pij daemon`, `path` and `telegram` refuse `E-RS-UNPORTED`; there is no legacy-force escape.

## Daemon

```bash
pij-rs ping --json
pij-rs daemon --bind <host:port>           # foreground service; use an owned supervisor
```

Use native `pij-rs daemon --help` for the installed lifecycle grammar. Do not substitute the former `pij daemon start/stop/kill` commands or assume spawn auto-starts a daemon. Rust changes require a rebuilt binary and an **authorized** restart (§ C6), not tsx hot-reload. Production rebuild/bounce and acceptance are prime-owned. An unavailable daemon or bearer credential is a real error, never permission to inspect/mutate a legacy store.

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

Reap is stale-record reconciliation, not process termination or age-based retention. A candidate needs confirmed dead/recycled process identity **and** confirmed absence of its recorded pane. Paneless seats need confirmed dead/recycled process identity alone. Live process/pane evidence retains the seat; unknown process/pane evidence is `unverifiable` and brakes retirement. Removing an unknown-evidence check can only permit the same set or more reaps: a **brake**, not product policy. Dry-run changes no store/spine rows. Execution re-observes the binding before conditional atomic retirement, protecting a concurrently revived incarnation. Never signal a recycled pid.

**Exact ps absence protocol**: `LC_ALL=C ps -o lstart=PIJ_LSTART -p <pid>` returns exit 1, empty stderr, and one UTF-8 header line trimming to `PIJ_LSTART`, with no subsequent line. Empty stdout is not absence. Exit 0 needs header + exactly one parseable process row. Extra blank/data rows, any stderr (including whitespace), wrong header, other exits, signals and parse failures are unknown. Source and named existing tests: [`crates/harnesses/src/proc.rs`](../../../../crates/harnesses/src/proc.rs), especially `exit_one_with_header_only_and_empty_stderr_is_absent` and `failed_probe_requires_exact_header_only_output`. These names are not a claim of a new validation run.

## Recover after a reboot

Re-derive every process/pane observation; old numbers belong to a previous epoch. Start with native ping and rs records. Native revive is explicit:

```bash
pij-rs revive <tombstoned-seat> [--session <tmux-session>] [--name <window-name>] --json
```

It relaunches from stored identity/launch intent. It is **not** the former `--print`, `--attach` or `--assume-dead` workflow, and requires a known tombstoned id. A refusal is not permission to copy an archived TS descriptor into rs, guess a prime, or start a fresh session wearing an old identity. In-place reattachment and folder-based prime revival remain named gaps. Human choice of successor/launch is still required where ownership or identity cannot be established. A revived peer is pending canary until the real recall/runtime/round-trip evidence is observed (§ C2).

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
| Government files unavailable | Resolve standing `prime-governance` worktree per bootstrap; never substitute main |

Government documents remain at the resolved standing orphan worktree. The current pij example is `$HOME/pi-hacking/pij-worktrees/pij-governance`, resolved by `harness/scripts/government-root.sh`; it is not a product/stream write target. Production and chainglass confirmation remain prime-owned.
