# Original ask

Brief from prime pij-far-jackal (2026-11-17), copied verbatim, then the prime's three amendments the same day.

## Plan 227 brief: fleet metro map as a pij utility

**From:** prime pij-far-jackal (pij-fleet). **Ask:** Jordan, 2026-11-17, relayed by OSK's prime pij-mutual-crane:
"Integrate it into the main pij repo, and add a utility to that repo, available from the `/pij` skill, to create
this, including any templates that are needed." Plan number 227 is claimed for you (`pij claims`).

## Read first (fully)
- OSK handover: `<OSK governance worktree>/.harness/government/fleet-map/HANDOVER-pij.md`
- The POC beside it: `index.html`, `generate.mjs`, `fleet-map.structure.json`, `README.md`, `refresh.sh`.
  The page's 17 requirements in the handover are all built. Keep every one.
- Read-only: never edit OSK files.

## Your job
1. Worktree from latest `origin/main`: `~/pi-hacking/pij-227-fleet-map`, branch `227-fleet-map`.
2. Run `/builder` for plan 227 in `docs/plans/227-fleet-map/`: plan, then tasks, then implement. Copy this brief in
   as `original-ask.md`. Load both the pij and builder skills.
3. Send me the plan (path plus a 5-line summary) **before implementing**. I approve or amend it within the turn.
4. Implement, then run `harness checks --quick` and the new tests, push, open the PR, and run `just pr-freeze <n>`.
   Tell me when it is FINAL.

## Decisions already made (prime)
- **Home:** `skills/pij/tools/fleet-map/` holds `generate.mjs`, `templates/index.html`,
  `templates/structure.example.json` (a small generic starter, not OSK's data) and a short README. It ships through
  the skill link, so any repo's agents have it.
- **Verb:** `pij fleet-map init|generate|open`, routed by the node shim (`harness/scripts/pij-cli.cjs`) to the
  script. No Rust and no daemon change.
  - `init [--dir <d>]` writes the starter structure and the page into `<d>`. It refuses to overwrite an existing
    structure.
  - `generate [--structure <p>] [--out <d>] [--at <iso>] [--offline] [--project <slug>]` writes `fleet-map.data.js`
    and copies the page next to it.
  - `open` prints the `file://` URL. It opens the page only with `--launch`.
  - Generated files go to a git-ignored or out-of-tree directory. Never commit them to a product branch.
- **Data from pij first:** seats from `pij list --json`. The POC reads `.data.seats`, which is wrong: today
  `.data` is an array of arrays, so check the real shape and fix it. Roles from `pij directory --json --project`.
  Verdicts from `pij outputs --subject pr:N --head <sha> --json`.
  - Keep the POC's PR-comment verdict parser only as a fallback when pij has no output for that head.
    OSK still posts comment verdicts.
  - `gh pr view` is optional: it supplies state, head and title. When gh is missing or fails, it degrades to a
    warning in the data, never an error.
- **Known gaps:** fix out-of-order merges (solid track around a merged stop, a gap only at the open stop). Leave
  "who's awake vs bg jobs" and seeding the structure from workstreams as named follow-ups in the plan; don't build
  them.
- **Skill:** one row in `skills/pij/SKILL.md`'s table, plus a short section in an existing route (ops or prime;
  your choice). Keep it token-lean: what it is, the three verbs, where the structure lives. No essays.

## Proof (acceptance)
- **Determinism:** a fixture structure in the repo run through `generate --offline --at <fixed>` must be
  byte-identical to a golden file. Running it twice gives the same bytes.
- **Fallback:** a test where pij outputs give the verdict and comments disagree. pij wins.
- **Smoke:** the live map renders for pij-fleet itself. Write a small `structure` for pij-fleet in the governance
  worktree (`harness/scripts/government-root.sh`) and open the page. Send me a screenshot or the path.
- **Cost row in the plan's backpressure:** it's a one-shot command. Per run: one `pij list`, one `pij directory`,
  and per PR one `gh pr view` plus one `pij outputs`. No loop, no daemon launches, no hook.

## Rules
- Builds and tests run under `nice -n 19`, one at a time. This is node-only work, so you shouldn't need cargo.
- Two review rounds, then ship (C34). Don't over-engineer. Keep the POC's page code largely as it is; make it a
  template, and don't rewrite it.
- Report to me (`pij send pij-far-jackal`) with normal sends: plan ready, blocked, question, FINAL. Use `--fyi` only
  for things I need do nothing about.

## Amendments (prime, 2026-11-17)

1. Correction: `pij list --json` DOES return `.data.seats`, as the POC reads it (crane checked: 74 seats). My brief was wrong; drop that "fix". Do verify the field names you read against real output.
2. Jordan's layout, make it the default: the hand-kept structure lives in the project's GOVERNANCE worktree (`harness/scripts/government-root.sh` in that repo gives `<government-root>/fleet-map/`), and `generate` copies the page plus data into `<main checkout>/scratch/fleet-map/` (git-ignored), because Jordan's chainglass client points only at main. So `pij fleet-map generate` with no flags, run in a project's repo, finds that project's governance structure and writes to its main `scratch/fleet-map/`. It warns if scratch/ isn't git-ignored and never commits there. `init` writes the starter structure into governance. Put this in the skill text. The second user is unimatrix (pij-loud-stoat builds its map once yours lands), so don't hardcode pij-fleet or OSK paths.
3. (Jordan) The skill copy must never be edited by accident.
   - `generate` and `init` COPY the page into scratch/governance, never symlink, so editing a project's copy can't reach the skill.
   - Both refuse any `--out`, `--dir` or `--structure` whose realpath is inside the skill folder (the folder generate.mjs lives in). Write a test for that refusal.
   - The colour write-back only ever writes the project's structure file, never the skill's example.
   - The generated page and data carry a header comment: "generated by pij fleet-map from skills/pij/tools/fleet-map; edits here are overwritten; change the template in the pij repo". Say the same in the README and the skill text.
