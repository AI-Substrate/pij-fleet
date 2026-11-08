# community — run a Pij Emergent Community (PEC)

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: run the project's community of roles. The prime and the humans build whatever community the work needs now and reshape it as the work changes (ruling C26). Templates are examples, not a checklist (C18): start from one when it fits, edit it freely, or write a role no template describes. Nothing is created for you (C22).

**Why**: a PEC builds lasting knowledge of its problem space. Specialist roles hold what the fleet learns, in memory and outputs that outlive any seat, and are called on later: revived, or a fresh seat that reads the role. Long-running specialists are a feature, not a cost to minimise away, and none of this is engineering-specific: a design workshop, research or operations grows a PEC the same way (C30).

A **workstream** orders PRD rows and blueprint pieces into steps (in `docs/project/workstreams/`; [how to write one](../community/workstream.md)); it nests (`part_of`) and maps to 0..n lanes. A **pij stream** is one lane of a workstream (C29).

The directory lives on the governance branch under `<government-root>/community/` ([where that is](../prime/rituals/bootstrap.md#governance-branch--the-rules)). Only pij's writer changes it: every verb below validates with ddocs and commits only its own paths. Never hand-edit a role, holders, output or memory document. `pij directory '<verb>' --help` works with no daemon. The API and refusals are in [Community directory](../../../../docs/how/pij-rs-api.md#community-directory-plan-190).

## Prime: build from needs, then reshape

```bash
pij directory [--suggest]                        # the prime, roles, actions, prompts, the boundary review; --suggest: every suggestion
pij directory templates [--show '<name>[@vN]']   # the shipped examples, and one template's body
pij directory add '<id>' --from 'template:<name>' --why '<the need>'  [flags override fields]
pij directory add '<id>' --from 'file:<brief>' --why '<the need>' --rule '<text>' ...
pij directory add '<id>' --why '<the need>' --title '<title>' --rule '<rule>' --packet '<packet>'  # no template
pij directory assign '<role>[/<instance>]' '<seat>' --why '<text>'
pij directory release '<role>[/<instance>]' '<seat>' --why '<text>'
pij directory edit '<id>' --why '<text>' --set '<field>=<value>' [role flags]
pij directory retire '<role>' --why '<text>' [--drop-queued]
pij directory merge '<from>' --into '<to>' --why '<text>' [--drop-queued]
pij directory template-diff '<id>'               # upstream change since the role's version; never applied
pij directory routes [--write --why '<text>']    # who owns which paths, with live holders
```

Every `<placeholder>` in a printed command sits inside single quotes: fill it there (write a `'` in the value as `'\''`) so the shell passes one literal word. Pass message and note bodies with `--body-file`.

`add --from template:` copies the template as it is. Replace the project's placeholders (`<project>`, `<government-root>`, `<workstream number>`, and a template's own, such as `<topic>`) with flags or a later `edit --set`; `<did>`, `<next>`, `<seat>` and `<path>` in a packet's commands are the holder's to fill at run time. Give real `--scope-path`s, since scopes become ROUTES.md. A repeated flag (`--rule`, `--scope-path`) replaces the template's list. Each packet sends its holder to `<government-root>/community/README.md`: write that once by hand (what the community is for, where its docs live).

1. **Start from the work.** Read the workstream's steps (their `needs` are roles; `depends_on` is the order), then the view. Its actions name a role with requests waiting and no live holder, two live holders on a one-holder role, or memory gone stale. Its prompts name roles a step `needs` (core ones included), paths no role owns and work aimed at a retired role; the other core roles and security wait under `--suggest`. Each prints runnable lines, and each prompt the signal it rests on. Add a role when a need has no owner, not because a template exists ([when to recommend one](#when-to-recommend-a-role-c28)).
2. **The core roles are suggested, not required.** Until each exists, those no step needs (of prime, pm, pa, coder, reviewer and security) are named on one "Suggested roles not yet here" line; `pij directory --suggest` prints each with its `add` command. A pm is kept per workstream: `pij directory add pm-001 --from template:pm --instance-key 001 --why 'workstream 001 needs a PM'`, the id the docs' `owner` fields name. A role counts when its id or its template matches (`pm-001` from `pm`). The prime is designated, never assigned (C24): the view's `prime` names its seat, and a prime prompt only adds the role's document.
3. **Reshape as needs shift.** `edit` changes a role in place (one changelog row each). `retire` ends a role and `merge <from> --into <to>` folds one into another; release live holders first, and a role with queued requests refuses `retire` and `merge` until you assign a holder to take them or pass `--drop-queued` (each requester is told). Nothing is deleted: memory, outputs and lineage stay readable, and sends to a retired role refuse. Reassign rather than respawn.
4. **Drift is advice.** A role whose template moved ahead is marked; read `template-diff` and copy what you want by hand.
5. **Borrowed seats**: `pij directory external add '<seat>' --from-project '<slug>' --why '<note>'`.
6. **Routing**: `routes` builds the table from role scopes into `community/ROUTES.md` in the same commit as each role change, and seats get it as an FYI when assigned. Keep scopes true when you reshape.
7. **Project docs** (PRDs, blueprints, workstreams, rulings): follow [the doc procedures](../community/README.md).

### When to recommend a role (C28)

Keep the fleet small but expert. Small contexts are cheaper and sharper, and an expert in its own seat beats a generalist carrying the topic. The prime and PMs recommend worker and specialist roles as the work reveals them; templates stay examples.

- **For a new role**: it would be worth remembering (C30); the same system or paths keep coming up across tasks; a finding is re-derived (the same one twice); a judgment must be independent of the author (security, performance); a seat is bloating because it carries two domains; a path or system needs one owner.
- **Against**: one-off work; overlap with an existing role (extend that one); nothing worth remembering; it would mostly sit idle.
- **A role is not a seat.** Roles can be many and cold: their memory, lineage and queue wait, and a holder is assigned when work arrives. Live seats stay few.
- **Review at boundaries** (wave end, compact, PR merge): the view's boundary review proposes splitting a seat that holds two roles and retiring (or merging) a role idle 30 days with nothing to remember. A cold role with memory is kept and called on (`pij send 'role:<id>' --body-file '<path>'`). Have each holder write memory at the boundary.
- **Name the signal.** Each recommendation the view prints ends `signal: <which>`; carry it into the `--why`: `--why 'same finding twice: pane reads in s07 and s09'`.

## PM: staff a workstream's steps

```bash
pij claim '<kind>' --for '<subject>'             # the next plan/migration/... number, never guessed
pij directory assign 'coder/<step>' '<seat>' --why '<step and plan>'
pij send 'role:<id>[/<instance>]' --body-file '<packet>' [--reply wake|fyi|none] [--limit 45m]
pij outputs --subject 'pr:<n>' --head '<sha>'    # verdicts at the exact head; none once it moves
pij directory release 'coder/<step>' '<seat>' --why merged
pij delivered '<pr>'                             # the merger, once per merge: prints the ddocs edits
```

- Send to a role, not a remembered seat: the directory resolves the holder, and a role with no holder queues the request and wakes the PA.
- Packets carry the work: paths, proofs, the report target, and the trailers. Write the packet to disk and send its path (dispatch invariant 2).
- Flip states in the PR that delivers them, with a `Delivers:` line in its body.
- A PR that fixes F-items says so on its own line, `Fixes: F119, F117` (each `F` and its number, comma-separated). `pij delivered` ends the `coder/f<N>` holdings it names; with no line, it ends none (it never guesses from the title).
- `pij delivered` ends the holdings its PR finished, but holdings for PRs merged before the project ran it stay open. Clear those once: `pij directory release --batch '<file>' --why '<reason>'`, with one `<role>[/<instance>] <seat>` per line, all in one commit.

## PA: memory before compacts

```bash
pij directory memory '<role>' [--note '<slug>']  # the index, or one note
pij directory memory add '<role>' --title '<t>' --hook '<one line>' [--tags '<a,b>'] --body-file '<path>'
```

- Before a role holder is compacted or closed (including one that released a role since its last compact), pij gives it a memory turn and holds the compact or close until that turn ends. Check the note landed and that its INDEX line reads well.
- The view's `memory-stale` action marks a role whose last 2 closes left no memory note, with the holder's `memory add` line. Nudge the holder; never write its memory for it.

## Every role holder

- Results are outputs, not chat: `pij output verdict --role '<id>' --subject '<address>' --head '<sha>' --result pass|pass-with-notes|block [--answers '<msg_id>']`, or `deliverable|note|handover` without `--head`. `--answers` tells the requester in the mode it asked for.
- Write a memory note before a compact or close, and read your role's `memory/INDEX.md` when you take a role.
- Your packet says what to do; this route does not.

## Failure modes

| Symptom | Move |
|---|---|
| `E-RS-GOV-ABSENT` | Restore the standing governance worktree as its `details.next` says; never recreate the branch |
| `E-RS-GOV-FOREIGN-EDIT` | Someone edited a path the writer needs: commit or remove that edit, then retry |
| `E-RS-DIRECTORY-ROLE-MISSING` on assign | Add the role first (`pij directory add '<role>' --why '<the need>'`) |
| A send to a role refuses with several holders | Name the instance: `'role:<id>/<instance>'` |
| A prompt you don't need | Ignore it; prompts never create anything |
