# Reports — evidence one hop up

Every report is a file inside the sender's fence, persisted before its pointer is
sent. File at preamble, each phase checkpoint, and ship.

## Contract

| Field | Required content |
|---|---|
| `claim` | One line naming exactly what is claimed |
| `artifacts[]` | Exact paths to plans, flows, logs, reviews, and outputs |
| `shas[]` | Commit or content hashes supporting the claim |
| `gates[]` | Exact command, verdict, and evidence-output path |
| `observations[]` | `id / kind / layer / one-liner / suggested encoding` |
| `open[]` | Unresolved decisions, risks, escalations, and skips |

Paths carry the detail; the report does not replace its artifacts. Send one
short pointer message after the file exists — per § C10 (wire discipline):
first line = the claim/verdict + the path, nothing restated from the report.
At plan close-out, include the named PRD rows advanced and receipt per row for the reconciliation owner (governance rules below); a report never grants a stream write access to government.

## Receiver duty

1. Treat every green, completion message, and claimed verification as a claim.
2. Read one load-bearing artifact or re-run one cheap gate from the report.
3. Record the verification note before acting, granting a baton, or relaying.
4. If an artifact is missing or contradicts the claim, reopen the report.
5. Transcribe observations into the receiver's own single-writer ledger.

A resumed subagent once claimed completion and invented an o-prime sign-off; the
owning stream looked at the file and caught both. That is why verification also
applies to statements that say "verified."

When a target can mutate during review, freeze it first: record its hash, verify
that exact version, and name the hash in the verdict.

When the claim rests on a **live/external resource under a baton** (a dev
store, a rate-limited API, a single app window): your probe is USE of that
resource — ask the HOLDER to re-run its sensor while you watch the artifact,
never fire side-band. On rate-limited externals a second independent prober
subtracts confidence: it can trip edge protection and forge the very fault
under investigation (a 503 masqueraded as the original bug and nearly
falsified a correct "fixed" report in the first outside run).

## Top-layer evidence and digest

If no layer exists above the o-prime:

- authoritative store records plus single-writer documents at `<government-root>`
  are the evidence; resolve document paths via
  [Governance branch — the rules](./bootstrap.md#governance-branch--the-rules);
- send the human only main-event digests: phase ships, stream unblocks or
  dissolutions, incidents/collisions, and rulings needed;
- keep digests short, phone-readable, and self-identified.

Numbered `prime-NNNN` reports resume only when an actual optional overseer exists
to receive and verify them. Do not create reports for a dead audience.
