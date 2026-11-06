# community — run a Pij Emergent Community (PEC)

> Route module — sibling-blind. Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: run the project's community of roles. The prime and the humans build whatever community the work needs now and reshape it as the work changes (ruling C26). Templates are examples, not a checklist (C18): start from one when it fits, edit it freely, or write a role no template describes. Nothing is created for you (C22).

**Why**: a PEC builds lasting knowledge of its problem space. Specialist roles hold what the fleet learns, in memory and outputs that outlive any seat, and are called on later: revived, or a fresh seat that reads the role. Long-running specialists are a feature, and none of this is engineering-specific: a design workshop, research or operations grows a PEC the same way (C30).

A **workstream** orders PRD rows and blueprint pieces into steps (filed under `docs/project/roadmaps/` until the rename); it nests (`part_of`) and maps to 0..n lanes. A **pij stream** is one lane of a workstream (C29).

The directory lives on the governance branch under `<government-root>/community/` ([where that is](../prime/rituals/bootstrap.md#governance-branch--the-rules)). Only pij's writer changes it: every verb below validates with ddocs and commits only its own paths. Never hand-edit a role, holders, output or memory document. `pij directory <verb> --help` works with no daemon. The API and refusals are in [Community directory](../../../../docs/how/pij-rs-api.md#community-directory-plan-190).

## Prime: build from needs, then reshape

```bash
pij directory                                    # roles, holders, drift, prompts (missing core roles, security, unmet needs)
pij directory templates [--show <name>[@vN]]     # the shipped examples, and one template's body
pij directory add <id> --from template:<name> --why "<the need>"  [flags override fields]
pij directory add <id> --from file:<brief> --why "<the need>" --rule <text> ...
pij directory add <id> --why "<the need>" --title ... --rule ... --packet ...   # a role no template describes
pij directory assign <role>[/<instance>] <seat> --why <text>
pij directory release <role>[/<instance>] <seat> --why <text>
pij directory template-diff <id>                 # upstream change since the role's version; never applied
```

1. **Start from the work.** Read the workstream's steps (their `needs` are roles; `depends_on` is the order), then the view's prompts. Each prompt prints a runnable line. Add a role when a need has no owner, not because a template exists ([when to recommend one](#when-to-recommend-a-role-c28)).
2. **The core roles are prompted, not required.** prime, pm, pa, coder and reviewer are prompted until each exists, and security until a security role exists. A role counts when its id or its template matches (`pm-001` from `pm`). The prime is designated, never assigned (C24).
3. **Reshape as needs shift.** Add a role for a new need. To merge two roles, add the merged one, assign it and release the old holders. To retire a role, release its holders and say why; its document and lineage stay as history. Reassign rather than respawn.
4. **Drift is advice.** A role whose template moved ahead is marked; read `template-diff` and copy what you want by hand.
5. **Borrowed seats**: `pij directory external add <seat> --from-project <slug> --why <note>`.
6. **Routing**: `pij directory routes` prints who owns which paths (generated from role scopes); the prime gets one FYI when it changes.
7. **Project docs** (PRDs, blueprints, workstreams, rulings): follow [the doc procedures](../community/README.md).

### When to recommend a role (C28)

Keep the fleet small but expert. Small contexts are cheaper and sharper, and an expert in its own seat beats a generalist carrying the topic. The prime and PMs recommend roles as the work reveals them; templates stay examples.

- **For a new role**: it would be worth remembering (C30); the same system or paths keep coming up across tasks; a finding is re-derived (the same one twice); a judgment must be independent of the author (security, performance); a seat is bloating because it carries two domains; a path or system needs one owner.
- **Against**: one-off work; overlap with an existing role (extend that one); nothing worth remembering; it would mostly sit idle.
- **A role is not a seat.** Roles can be many and cold: their memory, lineage and queue wait, and a holder is assigned when work arrives. Live seats stay few.
- **Review at boundaries** (wave end, compact, PR merge): split an overloaded role, merge or retire idle ones (step 3).
- **Name the signal.** Every recommendation says which signal it rests on, and the `--why` carries it: `--why "same finding twice: pane reads in s07 and s09"`.

## PM: staff a workstream's steps

```bash
pij claim <kind> --for <subject>                 # the next plan/migration/... number, never guessed
pij directory assign coder/<step> <seat> --why "<step and plan>"
pij send role:<id>[/<instance>] --body-file <packet> [--reply wake|fyi|none] [--limit 45m]
pij outputs --subject pr:<n> --head <sha>        # verdicts at the exact head; none once it moves
pij directory release coder/<step> <seat> --why merged
pij delivered <pr>                               # the merger, once per merge: prints the ddocs edits
```

- Send to a role, not a remembered seat: the directory resolves the holder, and a role with no holder queues the request and wakes the PA.
- Packets carry the work: paths, proofs, the report target, and the trailers. Write the packet to disk and send its path (dispatch invariant 2).
- Flip states in the PR that delivers them, with a `Delivers:` line in its body.

## PA: memory before compacts

```bash
pij directory memory <role> [--note <slug>]      # the index, or one note
pij directory memory add <role> --title <t> --tags <a,b> --hook <one line> --body-file <path>
```

- Before a role holder is compacted or closed, pij gives it a memory turn and holds the compact or close until that turn ends. Check the note landed and that its INDEX line reads well.
- The view flags a role whose memory went stale across recent closes. Nudge the holder; never write its memory for it.

## Every role holder

- Results are outputs, not chat: `pij output <kind> --role <id> --subject <address> [--answers <msg_id>] ...`. A verdict names the exact head, and `--answers` notifies the requester in the mode it asked for.
- Write a memory note before a compact or close, and read your role's `memory/INDEX.md` when you take a role.
- Your packet says what to do; this route does not.

## Failure modes

| Symptom | Move |
|---|---|
| `E-RS-GOV-ABSENT` | Restore the standing governance worktree as its `details.next` says; never recreate the branch |
| `E-RS-GOV-FOREIGN-EDIT` | Someone edited a path the writer needs: commit or remove that edit, then retry |
| `E-RS-DIRECTORY-ROLE-MISSING` on assign | Add the role first (`pij directory add`) |
| A send to a role refuses with several holders | Name the instance: `role:<id>/<instance>` |
| A prompt you don't need | Ignore it; prompts never create anything |
