# Fresh-prime canary (AC3, bp-0003)

Run 2026-11-07 by pij-following-marsupial (coder/204) at merge commit 7b003ce plus the route edits for s15, then the route fixed where the canary hesitated.

## Setup

- **Rig:** [`canary-rig.sh`](canary-rig.sh). It runs this branch's `pij-rs` as a private daemon on a private port and tmux server, with its own state dir and HOME. The scratch project `digest` is the daily-digest example's PRD, blueprint and workstream in a plain git repo, plus a standing `prime-governance` worktree. A wrapper makes `pij` this branch's `pij-rs`. Nothing touched the machine daemon's store, `~/.pij-rs` or the live governance branch (one read-only call did reach the machine daemon; see Rig notes).
- **Seat:** a fresh OMP session (`pij-premier-hyena`, Opus 5.5, OMP's default model), started in the project folder in the private tmux server. It registered with the private daemon and was designated the project's prime with `pij project set digest --prime`. It had no prior context.
- **Brief** (sent from a registered operator seat): be the prime of `digest` and stand up the community, the core roles plus security, shaped by what the work needs. Follow only the route at its path in a copy of this branch's skill, and files it links. Use the `pij` CLI as shown. Spawn no seats. Log every hesitation to `canary-notes.md`, and end with each role and the signal it rests on.

## Result

| Check | Result |
|---|---|
| Roles in the scratch directory | 7, one writer commit each: prime, pm-001, coder, reviewer, security, pa, performance |
| Core roles plus security | all present; performance added because workstream step s01 `needs: role:performance` |
| `ddocs validate` on each `roles/*/role.dd.json` (run by me) | ok on all 7 |
| `community/ROUTES.md` | generated in the same commits; no `<placeholder>` scope |
| Each role names its signal | yes, in the canary's table (core, step needs, independent judgment, worth remembering) |
| Read outside the route | none, apart from the project's own docs and `git log` on the governance worktree |

The canary's own notes are in [canary-notes.md](canary-notes.md), with scratch paths shown as `<scratch>`.

## Where it hesitated, and what changed in the route

| Hesitation | Fix |
|---|---|
| Item 1 said core roles wait under `--suggest` until a need points at them, but the plain view listed coder and reviewer (needed by s01/s02), so it expected only performance. | Item 1: prompts name roles a step `needs`, core ones included; the other core roles and security wait under `--suggest`. Item 2 says the same. |
| `pm` or `pm-001`, and whether `--instance-key 001` was needed. | Item 2: a pm is kept per workstream, `pij directory add pm-001 --from template:pm --instance-key 001`, the id the docs' `owner` fields name (as pij-fleet's own pm-001 is). |
| `add --from template:` left `<project>`, `<government-root>` and `<roadmap number>` in the packet. | New paragraph after the commands: `add --from template:` copies the template as it is; replace every placeholder with flags or a later `edit --set`. |
| Whether `--rule`/`--scope-path` append to or replace the template's lists. | Same paragraph: a repeated flag replaces the template's list. |
| The coder template's placeholder scope would reach ROUTES.md. | Same paragraph: give real `--scope-path`s, since scopes become ROUTES.md. |
| Every packet points at `<government-root>/community/README.md`, which nothing creates. | Same paragraph: write that README once by hand. |

## Recorded, not changed (product F-items for pm-001)

- `--suggest` prints `pij directory add pm …`, where the template and the docs want `pm-<NNN>`; it could read the workstream number.
- `add --from template:` could fill the placeholders pij knows (`<project>`, `<government-root>`).
- `pij directory add --help` doesn't say that a repeated flag replaces the template's list (`edit --help` does).
- `routes` writes a `<placeholder>` scope into ROUTES.md as if it were a path; it could refuse or flag one.
- A needed role with no template gets a suggested `add` with `--kind general` and every field a placeholder.
- The view lists `prime (general, one): no holder` under a header that names the designated prime, which reads as a contradiction (the prime is designated, never assigned, C24).
- Nothing creates `community/README.md`, though every template packet sends its holder there.

## Rig notes

- **One stray call reached the machine daemon.** The canary called the `pij` wrapper once from its tool kernel, which had no `PIJ_*` env: `pij directory templates --show pm`. That one read-only call went to the machine daemon (an older build) and was refused (`E-RS-ARG directory add: unsupported --show`). It then used bash for every call. The machine daemon has no `digest` project and no canary seat (checked after). The rig's wrapper now pins the private daemon itself.
- **The operator's reply was queued with "no harness session bound".** Its final `pij_send` to the operator showed this. The operator is a registered pane with no harness behind it; the canary's notes file was the report.
- **Not repeated after the fixes.** A second fresh seat run would confirm them; the route is 89/100 lines.
