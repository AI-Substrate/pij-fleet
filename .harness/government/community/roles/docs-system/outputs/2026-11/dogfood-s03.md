# Dogfood note: s03 (role templates, --from template, drift and PEC prompts)

docs-system (pij-wooden-lungfish), 2026-11-06 10:31 to 10:40 AEST, for pm-001. #126 merged at 1035c80; prod daemon pid 93273. All commands were read-only, run from the linked worktree `fleet-docs-system` (detached at 1035c80) with no `--project`. No role was created.

## What I ran

| Command | Result |
|---|---|
| `pij-rs directory` | 8 roles with their holders (assigned this morning), then the PEC guidance and one prompt: `prime (core): pij directory add prime --from template:prime --why "core role from the prime template" --project pij-fleet`. exit 0 |
| `pij-rs directory --json` | `data` keys `guidance`, `pending`, `project`, `prompts`, `roles`; `prompts` has the same single `prime` entry with its command |
| `pij-rs directory templates` | `6 role template(s); examples to adapt, not a checklist`: coder, pa, pm, prime, reviewer (each `[v1] core`) and security (`[v1]`, "Recommended for every community"). exit 0 |
| `pij-rs directory template-diff coder` | `coder: not made from a template`, exit 0 (same for `docs-system`) |
| `pij-rs directory template-diff pm-001` | `FAILED — E-RS-ARG role "pm-001" records template_version "", not a version number`, exit 2; `--json` gives `details.next: null` |
| `pij-rs directory template-diff nosuchrole` | `E-RS-DIRECTORY-ROLE-MISSING role "nosuchrole" has no readable .../roles/nosuchrole/role.dd.json`, exit 2 |
| `pij-rs directory --help`, `templates --help`, `template-diff --help` | all print usage with the PEC wording, exit 0 |

Cost: `git.community` stayed at 14 and `ddocs.community` at 4 across every command above. The read verbs launch nothing, as the docs say.

## What worked

- **The PEC prompts are right, and the wording is C26 and C18.** The view says: "Pij Emergent Communities (PEC): the prime and the humans build whatever community the work needs now, and reshape it as the work changes. The templates are examples, not a checklist: start from one when it fits and edit it freely, or write a role no template describes. Nothing here is created for you." It prompts and never creates (C22).
- **Instances count.** pij-fleet has no role with id `pm`, but `pm-001` (`template: pm`, `instance_key: 001`) satisfied the pm prompt, and `coder` satisfied coder. The only prompt is `prime`, which is correct: there is no prime role document. pm-001 expected prompts for prime, pm and coder by their exact ids; the view is smarter than that.
- **Security is offered as recommended, not core**, which matches C22.
- **The prompt hands you the exact command**, with `--project` filled in, and `--json` carries it as `command`.
- **Holders from this morning's assigns show up**, with instance keys (`pij-exuberant-wildcat (196)`) and the machine.
- `template-diff` on a role with no template gives a clear one-liner, and an unknown id names the file it looked for.

## What hurt

- **F19. template-diff refuses an imported role whose template has no version.** (medium) The wave B import gave `pm-001` `template: pm` (heron's convert.py, F8) but no `template_version`. `template_diff` reads the missing field as `""` (`crates/daemon/src/community/directory.rs:251`, `unwrap_or_default()`) and refuses with `E-RS-ARG role "pm-001" records template_version "", not a version number`. It's the wrong code (nothing is wrong with the caller's argument), it prints a missing field as an empty string, and `next` is null. Owner: plan 206 (lane E). Fix: treat a missing version as "made from template pm, version not recorded", and either diff from v1 with that caveat or say so and name how to record the version. If it stays a refusal, use a data code with a next step.
- **F20. No verb edits an existing role.** (medium) To fix F19, pm-001 needs `template_version: "1"` in its `role.dd.json`, but `add` refuses an existing role (`E-RS-DIRECTORY-EXISTS`, next step "choose another id"), roles are written only by the daemon, and there is no `set` or `edit` verb. So any role mistake is permanent unless someone hand-edits the governance branch, which the writer rule forbids. Reshaping (PRD 001 req-28: merge and retire) needs the same capability. Owner: plan 206 (lane E); PRD 001 req-28 needs it too. Fix: a `pij directory set <id> <field> <value> --why` (or `edit --file`) through the same writer, adding a changelog row.
- **F21. The view can't say who the prime is.** (low) The directory lists 8 roles and prompts to add a `prime` role, but pij-fleet has a prime (pij-far-jackal) that the view never names. A reader can't tell "no prime role document" from "no prime". Since the prime is designated and never assigned (C24), adding the role from the prompt won't show a holder either. Owner: plan 203 (s09). Fix: show the designated prime on the view's first line, and word the prompt as "record the prime's role" rather than a missing core role.
- **F22. Holder times are raw UTC timestamps.** (low) Each holder reads like `since 2026-11-05T23:09:45.254Z on <host>`. With five coders on one line, the coder row is about 400 characters, and the dates sit a day behind local time (the same family as F13). Owner: plan 203 (s09). Fix: a local date and time to the minute (`since 06 Nov 09:09`), one holder per line for `many` roles, and the machine only when it isn't this one.
- **F23. There's no way to read a template from the CLI.** (low) `templates` lists names, versions and summaries, and `--json` adds `title`, `kind`, `core` and `versions`, but neither gives a path or the body. So you can't see what `add --from template:pm` would write before you run it, unless you know the templates live at `templates/roles/<name>/v<n>/role.dd.json` in a pij checkout. Owner: plan 204 (s10). Fix: `pij directory templates show <name>[@v<n>]` (or a `path` field in `--json`).

## For the ledger

Renumbered by pm-001 to F19 to F23, since F17 (the two-role window name) and F18 (the writer and a hand commit racing on index.lock) were already taken. F17 to F23 go into `docs/difficulties.md` as D-WB-14 to D-WB-20 in the next docs PR, each naming its plan.
