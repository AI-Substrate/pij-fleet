# Packet: docs-system job 2, the PEC rename-forward docs PR (ruling C26)

From pm-001, 2026-11-06. Ruling C26 (Jordan, governance edf8125): the feature is now **Pij Emergent Communities (PEC)**. The prime and humans build whatever community the work needs now and reshape it as the work changes; templates stay examples (C18).

## Scope (rename forward only)
- PRD 001 (`docs/project/prds/001-pij-community.dd.json`): the title and summary say PEC. Read req-27 first: it covers "grow from needs" but not "reshape as the work changes". If it doesn't cover reshaping, add one requirement row (the next free id; check main) saying the community is emergent and reshaped as the work changes, state planned, with a testable done_when and a changelog row citing C26. Otherwise only reword.
- Blueprint 001 and roadmap 001: title and overview wording, with changelog rows citing C26.
- `skills/pij/references/community/` (README and procedures): name the feature PEC where they describe it.
- Governance `community/README.md` (in the standing governance worktree, NOT in the PR): the same wording. Edit it and send the prime the path to commit.
- Don't rename any file, slug or id (the `001-pij-community` slugs stay), and don't rewrite history or past changelog rows.
- Plan 194 (s03, coder pij-following-marsupial) already carries the PEC wording for its prompts and templates README, so leave `templates/` alone.

## Also in this PR
- Append to `docs/difficulties.md` the open dogfood frictions from your Third run (F12 to F14), plus two from my holder assigns: F15, past or closed seats can't enter lineage (assign needs a live seat, release needs a row), so HOLDERS.md history can't be migrated; and F16 (low), there's no batch assign, so recording 8 holders took 8 commits. Give each a severity, and name s04 follow-ups as the owner.

## Process
One PR from origin/main. Biome-format docs/project JSON after ddocs writes; `just ddocs-check`, biome and local-path-check pass. Send me the PR number and head; I assign the reviewer and security. Don't push after that unless a verdict asks.
