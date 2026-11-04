reviewer: block (head 99d8bf42102a8d642a520fbc27008369de63f8e5)

Reviewer: pij-estimated-chinchilla. Basis: C19, C21 and pm-001's explicit three-file scrub packet. Reviewed against merge-base 795c9b7643641ebc4773bb183288309f7aa04daf. Scope is three Markdown files, five additions and five deletions. The detached worktree stayed at the assigned head and remained clean.

PR comment: https://github.com/AI-Substrate/pij-fleet/pull/116#issuecomment-6622309660

## F001: P2, one occurrence of the scrubbed worktree path remains

`docs/plans/175-pij-skill-realign/assets/findings.md:78`, row `ro-22`, still quotes the exact home-relative worktree path removed from that file's `pr-04` row and from the two survey passages. Across all three complete files, the source-derived path occurs four times before the patch and once afterward.

Generalize the historical path in `ro-22` as well, preserving the row's stale-example finding and R2 disposition. This blocks the packet's explicit requirement that the path be absent from these files. It is not a request to rewrite history or remove product references permitted by C21; security owns its separate privacy verdict.

## Checks that pass

- `AGENTS.md:76-91` retains the worked example's meaning. Removing the mtime grace can only enlarge the eligible deletion set: it is a **brake**, not a retention policy. Pane absence still decides deletion. The two former example references are gone.
- `findings.md:135` and `survey/prime-rituals.md:11,27` remain coherent as historical survey findings. Their stale-government-example explanation and disposition survive.
- Full-file, case-insensitive scans find zero private owner/name identities at the reviewed head, down from four occurrences. The source identity was derived from the source checkout's origin at runtime; neither source text nor its identity was printed or persisted.
- Eight in-memory positive controls pass: identity and removed-path injection into each of the three files, plus each of the two removed example references. The counts-only receipt identifies the remaining path at line 78 without reproducing it.
- `git diff --check origin/main...99d8bf42102a8d642a520fbc27008369de63f8e5`: exit 0.
- `gh run view 41770826107 --json headSha,status,conclusion,jobs,url`: completed, success, exact reviewed SHA. `check (22)`, `check (24)`, `rust-lint` and `rust-test` all succeeded. Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/41770826107
- No loop, poll, timer, hook, sweep or process-launch behavior changes. No runtime cost delta.

## Local gate limits

`harness boot --json`: exit 0 with advisory `ready: false`; linked-worktree Vitest is missing, so readiness is not proven.

`harness checks --quick --json`: exit 1. Six local tooling/configuration stages fail (`local-paths`, `ddocs`, `typecheck`, `lockfile`, `lint`, `test`); the worktree lacks TS tools and Vitest, and ambient Biome rejects the repository's newer configuration. Native Copilot contracts pass. Attribution is unavailable. Rust and smoke were skipped. These are not claimed as green local checks or additional PR regressions. CI's ddocs warning mode is not proof of local ddocs validation.

No dependency installation/linking, Cargo command, build, daemon change, commit or push was performed.

## Evidence and harness feedback

Session artifacts: `review-116-scrub.py` and `review-116-scrub.json`. Running the Python probe exits 1 with one current-head path violation; all eight controls succeed. The script reads immutable Git blobs, verifies the worktree matches them, and prints counts and file/line locations only.

**magicWand:** a counts-only current-head documentation scrub sensor with runtime-derived identities, source-derived path controls and explicit C21 exceptions. Existing operational-path checks do not cover these Markdown surfaces. The preserved probe is a concrete starting point, not a product change in this read-only review.

**difficulties:** local composite checks cannot prove this worktree ready without the dependency setup forbidden by the packet. Observation `SUGG-001` was captured in the reviewer worktree's ignored harness buffer; the finding, controls and improvement request are preserved here for PM disposition.
