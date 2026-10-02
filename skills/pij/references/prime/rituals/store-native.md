# Store-native governance — the ruled default

**Ruling (Jordan, 2026-07-18)**: no engineered migrations. Fresh primes govern in
the pij platform store natively from day zero; existing primes **self-migrate
lazily** at seat-change or skill-refresh — each carries its own load-bearing
facts into the store as it works. Prose government stands as historical record;
a prose-governed repo keeps prose as the live surface until ITS prime migrates.
Documents still follow [Governance branch — the rules](./bootstrap.md#governance-branch--the-rules); resolve `<government-root>` there. The orphan branch changes document storage, not platform-store authority.

## The mapping — prose artifact → store surface

| Prose (legacy) | Store verb (attributed, append-only) |
|---|---|
| spine.md Event paragraph | `pij spine append --kind <kind> [--project <slug>] [--refs <seq,seq>]`; optional `--actor` must match the daemon-resolved caller |
| prime-flow.json portfolio node | `pij project create "<description>"` then `pij project set <slug> [--plan <path>] [--prime <id>]` |
| roster / assignment rows | `pij task set <id> "<task>" --project <slug>` · the assigned seat runs `pij report state <state>` |

**Always pass `--project` on `task set`**: the assignment carries the slug and
every later state-set/state-verified event inherits it — omit it and the
claim/verify lifecycle is silently orphaned from the project's filtered spine.
| verification of done | the current recorded parent runs `pij report verify <id>`; receipt binds the latest done sequence, never self-verification |
| seat/liveness truth | `pij list --json` · `pij node show <id> --json` · `pij anomalies --json` (rs only; list supports boolean `--here` and exact harness/folder/parent/scope=local forwarding, not legacy role/prime/archive filters; tree/link are unported) |
| human-readable spine | `pij spine render` returns `{text,cursor}`; it never writes a legacy spine file |

Read-back: `pij spine events [--since <seq>] [--project <slug>] [--peer <id>]`,
`pij project list|show <slug>`.

## Store facts to respect

- **Events are kind-coded records, not prose.** No body field exists. Narrative
  belongs in briefs/plan docs on disk (pointer delivery unchanged); link the
  record via `--project` and `--refs`. Refs take free-form pointer strings —
  convention: prior event seqs, `commit:<sha>`, `pr:<n>`, `path:<repo-relative>`
  — so a ruling/ship event CAN carry its on-disk pointer.
- **Append is immediate and irreversible** — no dry-run. The rs grammar accepts a literal kind with optional refs/project; `--bare` and append `--peer` are not supported. Prefer linked, meaningful events by protocol rather than claiming a nonexistent kind-only guard. Correct a mistaken event with a referenced event, never deletion.
- **Attribution is mandatory and honest** — omit `--actor` and the store records
  your seat id; never write as `"operator"`.
- Platform `seq` is store-allocated. Never encode your own ordering into it.
- Canonical wire fixtures live in `crates/testkit/fixtures/golden/api/`; they are not new live captures. Event payload is a JSON string; Hello is `{build,hello:true,v:1}` and replay cursors are machine maps. [Stable API / gaps](../../../../../docs/how/pij-rs-api.md) owns these consumer contracts.
- Roles are joined assertions, not authority. Prime designation and baton leases are separate rs records; no legacy fallback or direct filesystem mutation repairs them.

## Event-kind palette (recommended, not enforced — o-prime ruling 2026-07-18)

`kind` is open vocabulary (WS-5), but cross-prime queryability erodes when
primes coin near-synonyms (live census: ~30 kinds where
verify/report-verified/review-verdict/phase-approved all meant "verified").
**Reach for a CORE kind before coining one; coin freely only for a genuinely
new act.**

Core: `ruling` (o-prime decision) · `grant` (authority/baton/resume) ·
`ship` (landed to main) · `deploy` (machine-wide activation step) ·
`verify` (independent verification passed) · `finding` (routed dogfood/review
finding) · `blocker` (recorded impediment) · `transition` (stream/seat state
change) · `note` (governance telemetry).

Append is irreversible — no junk kinds (a stray `x` is permanent); correct a
mis-kinded event with a `--refs` correction, never a delete.

## Day-zero (fresh prime)

Replace ONLY bootstrap §3's prose portfolio/roster scaffold: `pij project create`
per human-named item, `project set` for plan path/prime, then spine events at each
governance act (grant, transition, ruling pointer, teardown). Keep §3's orphan
branch/permanent worktree for briefs, canaries and other documents. Seat proof,
per-repo contract, orient stack, and intake remain unchanged.

## Self-migration at seat-change (the lazy rule)

**Migrate-as-you-touch, never bulk-backfill.** When the incoming seat first
needs a fact — an active stream, a baton hold, a pending decision — it creates
that fact's store record (project / spine event / task+state), then acts on it.
Facts nobody touches stay prose-historical; that is correct, not a gap. Mark
migrated prose sections with a pointer line to the store record; never erase.
