# Role templates

These support **Pij Emergent Communities (PEC)** (ruling C26): the prime and the humans build whatever community the work needs now, and reshape it as the work changes, adding, merging or retiring roles as needs shift. The templates are examples of `pij/role` documents to adapt, **not a checklist** (ruling C18). Start from a template when one fits, change it to fit, and write a new role from scratch when no template does. Several of your roles may look nothing like these.

A PEC exists to build lasting knowledge of its problem space (ruling C30). Specialist roles hold what the fleet learns, in role memory and outputs that outlive any seat, so a role can be called on later: revived, or taken by a fresh seat that reads it. Long-running specialists are a feature, not a cost. Nothing here is engineering-specific: a design workshop, a research effort or an operations team grows its roles the same way, and "this would be worth remembering" is a reason to add one (C28).

| Template | Kind | Holders | What it's for |
|---|---|---|---|
| [prime](prime/v3/role.dd.json) | general | one | Leads the community: the directory, rulings, routing, merges. |
| [pm](pm/v4/role.dd.json) | general | one per top-level workstream | Runs one workstream: packets, assignments, numbers, true states. |
| [pa](pa/v3/role.dd.json) | general | one | Keeps seats healthy: memory before compaction, no silent stalls. |
| [coder](coder/v4/role.dd.json) | general, per task | many | Builds one step from its packet and opens a PR. |
| [reviewer](reviewer/v3/role.dd.json) | general, per task | many | Reviews one PR for correctness at its exact head. |
| [security](security/v3/role.dd.json) | specialist | one | A security verdict on every PR (recommended for every community). |

### Optional examples

Roles some communities need and others never will. Each is an example of one way to shape the role; add one only when the work has that need, and change it to fit.

| Template | Kind | Holders | What it's for |
|---|---|---|---|
| [devcontainer](devcontainer/v1/role.dd.json) | specialist | one | Keeps the development container and toolchain building and current. |
| [researcher](researcher/v1/role.dd.json) | general, per task | many | Answers one research question with sources. |
| [note-taker](note-taker/v1/role.dd.json) | specialist | one | Records decisions, open questions and actions. |
| [topic-expert](topic-expert/v2/role.dd.json) | specialist | one | Advises on one subject and reviews changes to it; add one role per topic (`<topic>-expert`). |
| [docs-keeper](docs-keeper/v1/role.dd.json) | specialist | one | Keeps user and contributor docs true to the code. |
| [infra](infra/v1/role.dd.json) | specialist | one | Keeps CI, build machines and shared services working. |
| [release](release/v1/role.dd.json) | specialist | one | Prepares releases; publishes only with the operator's go. |
| [qa](qa/v1/role.dd.json) | general, per task | many | Exercises a change end to end and gives a verdict. |
| [designer](designer/v1/role.dd.json) | specialist | one | Designs user-facing interfaces and reviews changes to them. |
| [transcriber](transcriber/v1/role.dd.json) | general, per task | many | Turns a recording or dictation into clean, attributed text. |

prime, pm, pa, coder and reviewer are the core roles `pij directory` prompts for until each exists, and security is prompted until a security role exists (ruling C22). A role counts when its id or its template is that name, so `pm-001` made from `pm` clears the pm prompt. Each prompt prints a runnable `pij directory add` line; nothing is created automatically.

**prime is designated, not assigned.** Its template describes the prime's duties for the directory; `pij directory assign` never applies to it (ruling C24).

## Placeholders

Text in `<angle brackets>` is project-specific: replace the project's placeholders (`<project>`, `<government-root>`, `<workstream number>`, `<the paths named in the step's packet>`, and a template's own, such as `<topic>`) when you add the role. The commands in each packet (`pij report now '<did>' '<next>'`, `pij send '<seat>' --body-file '<path>'`) keep theirs: the holder fills them at run time, inside the single quotes (a `'` in the value is written `'\''`), so the shell passes each as one literal word, and sends text with `--body-file`. The `default_seat` names only a harness; pick the model and effort when you assign. The `default_limit` values are starting points for the time-limit check.

## Versions

Each version lives in its own folder (`<name>/v1/`, `<name>/v2/`, …) and is never edited after it ships, so a role made from `<name>@vN` can be compared with the latest version. A role records the template and version it came from in `meta.template` and `meta.template_version`. A new version is a new folder plus one row in `SHIPPED` in `crates/daemon/src/community/templates.rs`; a test fails until every file here is compiled in, and another pins each shipped version's bytes, so editing one fails. The six core templates are at v3: v2 single-quoted the placeholders in their packet commands, and v3 says "workstream" where v2 said "roadmap" (ruling C29), with paths under `docs/project/` unchanged. pm is at v4, which changes only its path to `docs/project/workstreams/` after plan 210 moved the folder. coder is at v4, which adds the PR body's `Delivers:` line and, for F-item fixes, a `Fixes:` line (F122), which `pij delivered` reads.

## Using one

`pij directory templates` lists the templates and their versions, and `pij directory templates --show <name>[@vN]` prints one template's body before you use it. `pij directory add <id> --from template:<name>` copies the latest version (`@v<n>` picks one) onto the governance branch with `meta.id` set to `<id>` and `meta.template`/`meta.template_version` recording where it came from; flags given alongside override the template's fields, as they do for any `add`. The writer validates the role and starts its changelog with your `--why`. Replace the placeholders with flags on the same add (`--title`, `--instance-key`, `--rule`, `--scope-path`, `--packet`), or edit the role on the governance branch afterwards.

Every version is compiled into pij, so a template needs no install. When a template moves ahead of the version a role came from, `pij directory` marks the role and `pij directory template-diff <id>` prints what changed upstream. Nothing is applied: the role keeps its own edits, and you copy any change you want by hand.
