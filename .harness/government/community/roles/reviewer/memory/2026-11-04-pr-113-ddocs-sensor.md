# PR 113 ddocs sensor review

Tags: reviewer, ddocs, traversal, mutation, exact-head

Hook: testing one broken file is insufficient when the scanner can silently lose a whole tree.

Verdict: block at assigned `75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb`. Report: `../outputs/2026-11/pr-113-verdict.md`.

F001, P2: `harness/scripts/ddocs-check.ts:126-127` catches all directory-read failures and returns an empty list. A known-invalid project PRD fails with missing required `why`; chmod 000 on scratch `docs/project` then makes the same sensor pass on only three examples. Restore permissions, failure returns; restore bytes, six documents pass. Fix discovery errors as named failures and regress mixed healthy/unreadable roots.

Seven pure unit tests pass. Fourteen real CLI controls cover clean, dangling link, dangling link with CI warn enabled, render drift, restoration, unavailable, CI unavailable annotation, relative override, invalid absolute override, no documents, broken project document, unreadable-tree silent pass, readable broken document, final restoration. Shared config was necessary: `vitest run --config <main>/vitest.config.ts --root <review> harness/scripts/ddocs-check.test.ts`.

The assigned worktree moved externally to `2b328dc6b71788fee466a0a8c1e0b7fc9720df1d` at 13:21:02, according to reflog. Source/tests/config/wiring were identical, but plan and PRD changed. Reran every probe with `git archive` of the full assigned SHA. Sensor blob `3047b271cd26b79ad84835465f833f4105445a9e` matches both heads. Do not implicitly grade newer heads.

C20 confirms CI's unavailable warning choice. Warning mode does not suppress actual validation failures. Wiring and minimal CI permissions/install diff are correct. No security/cost verdict claimed.

Shared Biome, local-path and lockfile checks pass. Boot is not ready; quick checks fail six tooling stages in the deliberately dependency-free worktree. No install or linking was attempted. Exact-head CI `41766405307` was cancelled, rust-lint passed, three jobs cancelled. No full local gate was claimed.

magicWand: permanent CLI regression for unreadable-tree plus healthy-other-tree discovery.

Difficulties DL-001 to DL-003 were captured in the ignored harness buffer: shared-config requirement, discovery's silent success, externally moving review checkout. Script and JSON receipts remain in this review session's files as `pr113-probes.mjs` and `pr113-probes.json`; the script pins extraction to the full packet SHA. Product edits remain forbidden for this reviewer.

Posted and read back PR comment `6621738610`, whose first line names the assigned full head and whose body includes F001. Required normal report send to `pij-steady-otter` was accepted with queued extension-stream receipt `17d8911e340d910b-00000000000008aa`. This proves send acceptance, not PM consumption. Scratch fixtures were removed; evidence script, receipts and immutable computed diff remain in session artifacts. No source changes by this reviewer.

## Reassigned frozen head 2b328dc

PM-001 confirmed it moved the checkout and explicitly requested the verdict at `2b328dc6b71788fee466a0a8c1e0b7fc9720df1d`. Reviewed the complete six-file, +41/-2 documentation delta: C18 becomes planned PRD req-27 with source, s03/s09/s10 note and changelog row; plan AC6/bp-0006 and their generated faces match. No implementation, tests, config, wiring or CI changed. The test blob also matches (`a7e5b22d031bd94042efe4218d65ab819276e8cf`).

Verdict remains block for F001 on the identical sensor. Fresh current-head six-document sensor run passes; changed plan and backpressure validate 0/0 and build --check reports no drift; PRD Biome passes. Exact-head CI `41766527358` passed all four jobs. Prior mutation evidence is explicitly attributed to 75cf1a1, not falsely claimed rerun. Updated report and log to name the new full head; original verdict preserved in session artifacts as `pr113-verdict-75cf1a1.md`. Posted and read back PR comment `6621778462`. Required normal report send to pm-001 accepted with queued extension-stream receipt `17d8911e340d910b-00000000000008b3` (acceptance, not consumption).
