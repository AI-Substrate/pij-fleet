# Role templates

Examples of `pij/role` documents to adapt, **not a checklist** (ruling C18). A community's roles come from what its work needs: start from a template when one fits, change it to fit, and write a new role from scratch when no template does. Several of your roles may look nothing like these.

| Template | Kind | Holders | What it's for |
|---|---|---|---|
| [prime](prime/v1/role.dd.json) | general | one | Leads the community: the directory, rulings, routing, merges. |
| [pm](pm/v1/role.dd.json) | general | one per roadmap | Runs one roadmap: packets, assignments, numbers, true states. |
| [pa](pa/v1/role.dd.json) | general | one | Keeps seats healthy: memory before compaction, no silent stalls. |
| [coder](coder/v1/role.dd.json) | general, per task | many | Builds one step from its packet and opens a PR. |
| [reviewer](reviewer/v1/role.dd.json) | general, per task | many | Reviews one PR for correctness at its exact head. |
| [security](security/v1/role.dd.json) | specialist | one | A security verdict on every PR (recommended for every community). |

prime, pm, pa, coder and reviewer are the core roles a new community is prompted for. security is prompted until a security role exists (ruling C22). Nothing is created automatically.

**prime is designated, not assigned.** Its template describes the prime's duties for the directory; `pij directory assign` never applies to it (ruling C24).

## Placeholders

Text in `<angle brackets>` is project-specific and must be replaced: `<project>`, `<roadmap number>`, `<the paths named in the step's packet>`. The `default_seat` names only a harness; pick the model and effort when you assign. The `default_limit` values are starting points for the time-limit check.

## Versions

Each version lives in its own folder (`<name>/v1/`, `<name>/v2/`, …) and is never edited after it ships, so a role made from `<name>@vN` can be compared with the latest version. A role records the template and version it came from in `meta.template` and `meta.template_version`.

## Using one

Once plan 194's second phase ships: `pij directory add <id> --from template:<name>` (optionally `@v<n>`), then override fields with flags as `pij directory add` already allows. Until then, copy a template, replace the placeholders and the id, and give it to `pij directory add --file`. The writer validates it and adds a changelog row for the add.
