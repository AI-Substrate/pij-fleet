# Ritual — incidents (detect → record → verify-repair → name-cause → rule → broadcast → encode)

> Rung 3. Loaded when something just went wrong across seats — a delivery wedge,
> a collision, a race, a swept file. The distillates elsewhere in this payload
> are what past incidents *taught*; this page is the practice that turns the
> next one into a lesson instead of a scar.

An incident is any cross-seat surprise where state, delivery, or trust briefly
went wrong — even if repaired in seconds, even if nothing was lost. Run-01
numbered four in two days (a stale descriptor that wedged ALL delivery; a
human-go racing governance; a compositor freeze; a bare `git commit` that swept
24 of a sibling's staged files). Every one was surfaced by its own causer, and
every one shipped a standing rule the same hour.

## The lifecycle (each step has an artifact)

1. **Detect and STOP the bleeding** — the operator move first, the paperwork
   second. Anyone may detect; the causer detecting their own is the norm, not
   the exception.
2. **Record at detection**: open `<government-root>/incidents/INC-NNN.md`
   (numbered, never recycled) with: what was observed · when · who/what was affected ·
   current containment state. Write it while it is happening — pass-time
   records, same law as canaries. Resolve `<government-root>` via
   [Governance branch — the rules](./bootstrap.md#governance-branch--the-rules).
3. **Self-report is CREDITED, never punished.** The run's traceability worked
   *only* because causers confessed (the benign baton breach, the swept
   commit). A seat that reports its own incident is doing the system's most
   valuable work; say so in the record. Punish concealment, never disclosure.
4. **Verify the repair MECHANICALLY before closing** — the o-prime re-runs the
   claim, never accepts it (INC-004's closer re-counted the swept files
   against the orphan commit). A repair claim without its check re-opens the
   incident.
5. **Name the root cause as a resource or protocol gap, never a blame line.**
   "The shared git index is an unserialized resource local commits don't
   baton" names a gap; "the coder committed carelessly" names a person and
   teaches nothing.
6. **Ship a standing rule SAME-HOUR and broadcast it** to every live stream
   (tree-push channel). A rule that waits for the retro protects nobody's
   afternoon.
7. **Encode with provenance**: the incident becomes an encode-candidate row
   (lesson → proposed encoding → INC-NNN receipt) riding the normal
   graduation path — ritual text, template, check, or tooling patch.

## Worked exemplar — INC-004, shared-tree fallback swept index (run-01, 08:12Z)

Before worktree-primary construction, a stream ran a bare `git commit` during a
sibling's apply window; the shared index swept **24 of the sibling's staged
files** into the commit. The committer detected it from the commit stat. Repair:
soft-reset preserving the victim's staged index, path-limited recommit, then a
closer re-counted the swept set. Root cause: index/HEAD was an unserialized
surface distinct from build locks. The standing rules remain mandatory whenever
a ruled shared-tree fallback is active: pathspec commits (`git commit -- <paths>`)
plus announce → ack → commit → confirm slots. The primary prevention is now a
separate worktree and branch per stream.

## Wiring into the rest of the government

- Delivery wedges: containment lives in
  [`bootstrap.md#recovery`](./bootstrap.md#recovery); this page owns the
  record/rule/encode tail.
- Resource collisions: the fix usually lands in
  [`batons.md`](./batons.md) — a new baton, a probe, or (INC-004) a
  serialized surface the book must name.
- The incident's one-liner rides the next report's `observations[]`
  ([`reports.md`](./reports.md)); the spine gets an event line at open AND
  close.
