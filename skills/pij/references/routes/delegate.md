# delegate — hand ONE bounded task to ONE peer

> Route module — sibling-blind. Knows only this job; composition is the dispatch's job.
> Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: get a single, bounded unit of work done by one colleague peer and take back the
result — **no review cycle, no verdict, no fleet**. One packet out, one done-report back,
teardown. If the work needs an independent cross-model reviewer and a fix loop, that's a
different, heavier job (a reviewed coder+reviewer fleet) — not this route. If you only want
to stand up or chat with a peer with no work product, that's the raw colleague seam. This
route is the thin **"one task → one peer → one result"** path between the two.

**Preconditions**: delivery ownership/identity per § C1, and the intended rs daemon answers `pij-rs ping --json`. Do not assume shim spawn auto-starts it; unported grammar refuses rather than running legacy.

## The single invariant that makes delegation safe: pointer + bounds

A delegation packet is **data written to disk first, then a pointer sent** — never a giant
inline body. Every packet states, at minimum:

1. **Mission** — the one task, in a sentence or two.
2. **Repo root** — the absolute path the peer works in.
3. **Allowed paths** — the *only* files/dirs the peer may create or modify.
4. **Forbidden paths** — enumerate at least `.the-flow-state.json`, `the-flow.json`,
   `the-flow.md`, and any ledger dir; the peer must never read or write them.
5. **Done-report shape** — what to send back (summary + files changed + gate results) and
   the id to send it to (you). Both the packet pointer message and the done-report follow
   § C10 (wire discipline): first line = outcome/next action; delta + ids, no restatement.

Persist the packet (e.g. under a `scratch/` or task dir), then send the **path pointer**
(§ C1 verb). Long context always travels as a file + pointer, never inline.

## Flow

```bash
# 1. acquire — provided-or-spawn ONE peer (§ C1 transport, § C5 placement)
pij-rs spawn --harness <h> --model <m> --cwd <absolute-worktree> --parent <your-seat> --json
#    canary-verify the footer + no-400 before trusting it (§ C2) — provided peers too

# 2. deliver — write the packet to disk, send the POINTER (never the body)
pij send <id> "Packet at: <rel-path> — read it fully, then implement + report."

# 3. collect — the daemon PUSHES the done-report back as an injected turn (§ C7);
#    do NOT poll or nudge. Do independent work while the peer runs.

# 4. retire an owned seat only when no longer reusable; no process/pane kill:
pij close <id> --json
```

- **One task, one run** — the packet authorizes the whole bounded task; the peer finishes it
  and reports once. It is not a multi-round review conversation.
- **Compact between tasks, reuse the peer (§ C3)** — for a *second* bounded task on the same
  peer, compact and re-deliver rather than close-and-respawn a healthy session.
- **No verdict, no Dim-0 gate** — delegation trusts the peer's done-report. If you need an
  independent cross-model reviewer proving the work non-vacuous, use the reviewed-fleet job.

## Done-report

The peer replies with its own report (typical shape):

```json
{ "outcome": "COMPLETE | PARTIAL | BLOCKED",
  "summary": "what was done + current state",
  "filesChanged": ["path/…"],
  "gatesClean": true,
  "notes": "blockers / decisions / questions" }
```

On `COMPLETE`: verify the gate results it claims (re-run the project's checks yourself when the
work is load-bearing — **you still own the outcome**), then teardown. On `PARTIAL`/`BLOCKED`:
read `notes`, then either re-delegate a narrowed packet or take it over.

## Failure modes

| Symptom | Move |
|---|---|
| Ready but inference fails | corroborate the exact model with the host catalog/footer and real canary (§ C2/C4); models shim is unported |
| No done-report for a long time | trust push (§ C7); inspect the actual host transcript/pane only for a known-broken transport, never native event tail as a transcript |
| Close refuses | only self/current recorded parent may tombstone; no `--force` escape, terminal teardown is separate |
