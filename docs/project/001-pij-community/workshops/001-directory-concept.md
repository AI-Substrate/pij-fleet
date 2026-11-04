# Workshop 001: the community directory

Status: agreed concept, 2026-11-04. Jordan and the pij prime (`pij-far-jackal`), after interviews with two seats that already run specialists: an o-prime governing a product repo, and a Copilot seat that ran a live design session.

## Why

A **pij community** is a project's agents operating as one cohesive fleet across many concurrent tasks. Its **directory** is what this workshop designs. The PRD, blueprint and roadmap that follow from it are `../prd.dd.json`, `../blueprint.dd.json` and `../../roadmaps/001-pij-community.dd.json`.

Split the work so each agent's context stays small and focused. Small contexts make agents sharper and faster, and they cost less, because cost grows roughly with the square of the context. A role that outlives its seats lets the fleet build knowledge over time: a project gets better at its own work the way a mining town gets good at mining. The village is the analogy; the vocabulary below is fleet language.

## Vocabulary

| Term | Meaning |
|---|---|
| **community** | a project's agents working as one fleet across many concurrent tasks |
| **directory** | one per community: every role, its scope, its holders, memory and outputs |
| **role** | a job in the project, with its own short id (`docker`, `security`, `coder`) |
| **specialist role** | a trade: devcontainer, security, researcher, note-taker, a topic expert |
| **general role** | prime, pm, pa, coder, reviewer |
| **lifecycle** | `long-lived` or `per-task`, set per role and independent of its kind |
| **holders** | `one` or `many` (coder, reviewer) |
| **seat** | unchanged: a pij seat with its pij id, such as `pij-mutual-crane` |
| **assign / release** | bind a seat to a role, or unbind it. `pij close` still tombstones the seat |
| **lineage** | every seat that has held a role, with session id and machine; each one is revivable |
| **role definition** | a `pij/role` ddoc: scope, inputs, outputs, forbidden paths, standing packet |
| **standing packet** | what a seat reads when it is assigned to a role |
| **role memory** | what anyone doing the job should know; it grows across the lineage |
| **output** | the result of one request: a verdict, a deliverable or a note |

## Decisions

1. **One directory per project, tied to the project path, like a prime.** Lending a seat to another project is not tracked. The prime may add an `external` entry as a plain note.
2. **It lives on the governance branch** under `<government-root>/community/`. If governance is not set up, pij refuses by name and points at the setup step.
3. **pij is the only writer, through its own verbs.** `pij directory add|scope|assign|release|memory`, `pij output`. pij writes the JSON, runs `ddocs validate` and `ddocs build` by absolute path, and commits on the governance branch. Nobody edits these files by hand. The database keeps a pointer (project id, repo-relative path, commit).
4. **ddocs is the format.** pij-rs reads `.dd.json` as plain JSON and uses the `ddocs` CLI to validate and build. A missing `ddocs` gives a named refusal, and `just doctor` checks for it. No Rust port of ddocs for now; flowspace3 has a read-only Rust parser if one is ever needed.
5. **Cross-machine work is out of scope, but nothing here may block it.** Git is the shared copy; each machine's database is a local index that can be rebuilt from the branch. Lineage records the machine. Transcripts never move between machines, because flowspace already holds conversation history.
6. **Anyone can change the directory; nobody polices it.** Every change records who made it and why, in the role's changelog. The skill gives the prime, the PM and the PA the duty to keep it tidy.
7. **Sending to a role:** `pij send role:<id>` delivers only when exactly one live seat holds the role. With 0 or 2+ live seats it refuses, and the refusal gives the next steps as runnable commands (revive the last holder, start a fresh seat assigned to the role, or pick one of the listed seats).
8. **Role memory is prompted automatically.** Before every compact and every close of a seat that holds a role, pij asks it to add what it learned to `community/roles/<id>/memory/`. That costs one turn at each of those moments, with no timer.
9. **Outputs are ddocs in git as well as store facts.** Path: `community/roles/<id>/outputs/<yyyy-mm>/<seq>-<kind>-<subject>.dd.json`, one file per output so nothing conflicts. The daemon writes them one at a time. flowspace indexes them. `pij outputs --role security --subject pr:204` is a database read. pij commits but never pushes; a project pushes its governance branch the way it already does.
10. **The requester chooses how it hears back.** `pij send role:x --reply wake|fyi|none`, default `wake`. The mode is stored on the request, and the output that answers it uses that mode. A `block` verdict or a `failed` result always wakes. An output nobody asked for goes as an FYI to the nearest seat above (its PM, else the prime). There is only ever one recipient.
11. **Mostly the prime's tool.** `pij directory` is the prime's operating picture: roles, holders, waiting requests, latest outputs, memory freshness. It also gives prompts, in the style of `harness flow orient`: "security has no live holder and 3 requests waiting", "docker has 2 live holders", "coder memory not updated in the last 5 closes". Worker seats just get told what to do: a standing packet, the role memory, and a task packet that says how to report with `pij output`.
12. **Role templates ship with pij.** `pij directory add security --from template:security` copies a versioned template onto the governance branch. The copy belongs to the project and is edited freely. `pij directory` shows when the template has moved ahead, with the diff, and never applies it automatically. Good local edits come back as template PRs on pij-fleet. Templates are written fresh, because pij-fleet is public.
    - **Core**, created with the directory: prime, pm, pa, coder, reviewer.
    - **Recommended**, and pij prompts until it exists: security.
    - **Optional:** devcontainer, researcher, note-taker, topic expert (blank), docs-keeper, infra, release, test and QA, designer, transcriber (per task, writes partial output as it goes).
13. **A routing table gets work to the owner.** Each role's scope names what it owns. pij builds a table ("send it, don't do it") from every scope, writes it to `community/ROUTES.md`, and puts it in front of seats:

    | Trigger | What is sent |
    |---|---|
    | a seat is assigned to a role | standing packet, role memory, routing table |
    | after a compact | the same three again |
    | a revived seat comes back | the routing table |
    | the routes change | only the changed rows, as an FYI to the project's live seats |
    | a seat asks | `pij directory routes` |

    A change of holder does not trigger a send. A stale holder name does no harm, because sending to a role with no live holder gets the actionable refusal.

## Examples

**Routing table, as a seat sees it**

```
Routing — <project> (directory @ 7c41e02)
Owned work: send it, don't do it. Reply modes: wake (default) | fyi | none.

  WHAT                                         SEND TO                HOLDER NOW
  .devcontainer/, Dockerfile*, compose*        role:devcontainer      pij-tidy-wren
  security review of any PR (verdict on head)  role:security          pij-quiet-canidae
  PRD / roadmap / blueprint state changes      role:docs-keeper       none live → pij revive pij-calm-ibis
  plan or roadmap questions                    your PM                send to your parent
```

**Devcontainer role: an output, and its memory**

```
pij output deliverable --subject req:pm-019/local-stack --path docker/compose.dev.yml \
  --head 3f9a2c1 --notes "postgres + redis + minio up; healthchecks green"
pij output verdict --subject pr:231 --head 8b07e4d --result pass-with-notes \
  --notes "image 1.4GB; multi-stage would cut it, not blocking"
```

```markdown
## Gotchas
- The Postgres healthcheck must use `pg_isready -U app`, or depends_on never turns healthy.
## Open items
- Multi-stage build for the api image (PR #231 note).
```

An output answers "what did you do for this request?". Memory answers "what should anyone doing this job know?".

## Open, for the next workshop

- **Budgets and heartbeats for per-task specialists.** A time or turn budget, forced partial writes, and a progress heartbeat. The design session's transcriber ran 75 minutes and 661 tool calls with nothing on disk.
- **The second part: prompting for PRDs, roadmaps and blueprints.** This includes how roles link to roadmap steps, so the directory can say "roadmap 019 needs docker work and there is no docker role".
- **Cost row.** Writes are per request, never per tick: about 2 ddocs launches plus git per output or directory edit. Measure on the live fleet when it is built.
- **Defaults taken without an explicit ruling:** core roles are created with the directory; a change to the routes is sent as an FYI to every live seat in the project.
