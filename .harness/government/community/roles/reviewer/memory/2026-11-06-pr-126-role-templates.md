# PR 126 role templates review

Tags: reviewer, templates, project-scope, mutation, C22, C24, PEC

Hook: replay a generated command from the original cwd; correct view data does not prove its suggested command keeps the selected project.

Block at 04144d1ca39130106202322d3479273c636a388a. Verdict: `../outputs/2026-11/pr-126-verdict.md`. F001 P2: `directory --project remote` prints an unscoped add-security command. Replaying it from fleet commits security in fleet, not remote. An explicitly scoped add correctly commits in remote. The new human template-diff hint drops project too. Required repair is to propagate and safely quote the view project in both command surfaces, with a two-project replay regression.

All five template HTTP tests pass, including real-ddocs writes for all six templates. Independent v1/latest content control passes. Ninety-three community/directory/native/Claude units, one core help test, four CLI coverage tests and fmt pass. Exact-source scratch embedding baseline and restore each pass six units; adding an unembedded role file triggers the intended guard. Six templates, three changed product docs and four plan/task/pressure docs validate 0/0; product renders match.

CI run 42132279610 passes all four jobs at the exact head. Local directory integration is 18 passed/1 failed: unchanged #122 same-second test fails its own precondition, also alone. Both holdings exist and second assign succeeds. The test waits for any millisecond timestamp change, not the next second; even a 792 ms run can cross a second. Not caused by this PR. All native and Claude load cases pass in the focused run.

Read-only checkout stayed pinned and clean. Own target only, incremental off, scoped builds; disk 124 to 122 GiB. No dependencies installed/linked, no production daemon or pane interaction, no commits/pushes. Boot NOT-READY and quick gate JS/tooling-limited; C25 means exact-head CI supplies JS proof.

magicWand: add a deterministic two-project replay contract for command-bearing views.

difficulties: boot recommends worktree-deps despite reviewer C25; capture DL-001 and SUGG-001 retained with the review. Evidence and isolated regression source live in this review session's files, referenced by the verdict.
