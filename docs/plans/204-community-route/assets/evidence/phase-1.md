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
