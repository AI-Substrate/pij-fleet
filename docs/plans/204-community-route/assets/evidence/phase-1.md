# Plan 204, phase 1: evidence

Final round on 2026-11-07, after s06 (#137), s08 (#130), s09 (#141) and s15 (#135) merged.

## What this round changed

- **Merged main** at bb753b8, keeping both sides:
  - the directory help (s15's `edit`, `retire` and `merge`, plus `templates --show`);
  - the directory leaf flags (s15's `--to` and `--write`, plus `--show`);
  - the leaf dispatch (`edit`, `retire`, `merge`, `routes`, `lineage`, plus `templates --show`);
  - the SKILL.md and pij-rs-api.md directory rows.
- **Route** (`skills/pij/references/routes/community.md`, 89/100 lines):
  - The prime's commands now include `edit`, `retire`, `merge --into`, `routes` and `--suggest`.
  - Item 3 teaches the shipped reshape verbs. Release live holders first; queued requests refuse until you assign or pass `--drop-queued`. Nothing is deleted.
  - Item 6 teaches `routes` and ROUTES.md.
  - The C28 block teaches the view's boundary review (two-domain split, 30-day idle retire or merge, cold roles with memory kept and called on) and `signal:` on every recommendation.
  - No "planned" or "not yet" wording remains.
- **Verb check, encoded:** `crates/cli/tests/skill_verb_coverage.rs::the_community_route_and_templates_print_real_commands`. Every `pij` command the route and the 16 templates print, in a fence or a backtick span, must name a pij-rs verb, and each `--flag` must appear in pij-rs's help for that verb or leaf. Negative controls are an unknown verb and an unknown flag. At this head: 29 commands, 0 problems.
- **Canary (AC3):** see [canary.md](canary.md). Seven roles were stood up from the route alone. The route was fixed at each hesitation, and the product gaps are recorded as F-items.
- **States:**
  - Roadmap 001 s10 is `complete`, with plan `docs/plans/204-community-route`. Its `delivers` gains req-27 and req-29 (C26 and C28's route half; no step delivered req-27 before).
  - PRD 001 req-15, req-27 and req-29 are `shipped` (req-13 was already shipped by s03).
  - Blueprint c-skill and c-templates are `shipped`.
  - Each of the three docs gets one changelog row.

## Gates (local, macOS, niced on the fleet-194 target)

- `just shared-build -- build -p pij-cli -p pij-daemon --bins` then `clippy -p pij-daemon -p pij-core -p pij-cli --all-targets -- -D warnings`: clean.
- `test -p pij-cli --test skill_verb_coverage`: 5 passed, including the new test.
- `bash harness/scripts/pij-skill-check.sh`: all green. The community route is within its budget at 89/100.
- `ddocs validate` on the roadmap, PRD and blueprint: ok. `ddocs-check`: 7 documents clean and current. `local-path-check`: clean.
- The full Rust and TS suites, and Linux clippy, run in CI on the head.

## Cost (AC5)

Docs, embedded templates and one test. No loop, poll, timer, hook or launch site added; `templates --show` reads the embedded catalog in-process.

## Review round 2 (verdicts at 560e260)

ocelot (performance) passed. guan (docs-system) passed with notes. bobcat (reviewer) blocked on F001 and F002, and squirrel (security) on S136001. All of it is folded into one push.

- **F001: the command check matched flags by substring against the union of help texts.** `skill_verb_coverage.rs` now:
  - checks exact `--flag` tokens against the selected leaf's own help;
  - checks the flags that leaf's `Usage:` lines require, choosing the lines whose literal words best match the command;
  - checks that every `<placeholder>` sits inside single quotes;
  - reads the latest version of each template.
  - `every_kind_of_bad_printed_command_is_caught` has eight one-fault controls: no verb, an unknown flag, a prefix (`--wh`), another leaf's flag (`templates --why`), a missing `--why` twice, an unquoted placeholder, and a double-quoted one. It also runs four good forms, including a bare verb mention.
  - One control first failed because the help's title line (`pij directory retire | merge: …`) was read as a usage line. Usage lines now come from the `Usage:` block only.
- **F002:** the pm example is `pij directory add pm-001 --from template:pm --instance-key 001 --why 'workstream 001 needs a PM'`.
- **S136001:** every dynamic argument the route prints is single-quoted, composite addresses included.
  - The route says once how to fill a quoted placeholder and that bodies go through `--body-file`.
  - The ten unshipped optional templates are quoted in place, with the same sentence in each packet.
  - The six shipped core templates keep v1 byte for byte and gain a v2 with only that change (pm-001's choice B; plan 210 makes v3 for the C29 rename).
  - `SHIPPED` now has 22 versions. `a_shipped_version_is_never_edited` pins every shipped version's sha256, so editing one fails and a new one adds a row.
  - The test also found two unquoted commands in the route's prose (`pij directory <verb> --help` and `pij send role:<id>`), now quoted.
- **Guan's notes:**
  - The placeholder sentence leaves `<did>`, `<next>`, `<seat>` and `<path>` to the holder (route and templates README).
  - Item 3 makes the role the thing that refuses `retire` and `merge`.
  - "each prompt", not each action, prints a signal.
- **The Copilot canary rerun and its rig fix** are in [canary.md](canary.md).

Local, macOS, niced on the fleet-194 target:
- `clippy -p pij-daemon -p pij-cli --all-targets -D warnings`: clean.
- `pij-daemon --lib community::templates`: 8 passed.
- `pij-daemon --test directory_http` (templates_http included): 62 passed.
- `pij-cli --test skill_verb_coverage`: 6 passed.
