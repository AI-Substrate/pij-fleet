reviewer: pass (head 279b0ec430edcb0b64ed39f76afbbb6ac111be20)

Reviewer: pij-estimated-chinchilla. Supersedes the block at 99d8bf42102a8d642a520fbc27008369de63f8e5 and corrects the provisional same-head block after pm-001 clarified the path rule. Reviewed the complete four-file, four-line fix delta in the new detached `fleet-review-116b` worktree. The original worktree stayed at the old head; both stayed clean. Basis: C19, C21 and the clarified packet. Security owns its separate verdict.

Prior comment: https://github.com/AI-Substrate/pij-fleet/pull/116#issuecomment-6622309660

Current comment: https://github.com/AI-Substrate/pij-fleet/pull/116#issuecomment-6622418995

## F001 closed

`findings.md:78` now generalizes the historical worktree example while retaining its R2 disposition. `survey/routes.md:57` makes the same coherent change. Plan 175 `ac-0008` and its generated Markdown generalize the dead path without changing the assertion's meaning, checked state, pressure or evidence links. The JSON's only semantic change is that assertion's claim text.

The counts-only probe now checks every complete file in the cumulative six-file PR and scans all tracked text at the exact head, including hidden paths. Private owner/name matches and the removed worktree path or suffix are zero across the tracked tree. Source-derived whole-tree positive controls find three identity-bearing lines and seven path-bearing lines at the base. The original brake-versus-policy example remains unchanged by the fix and retains its meaning.

## No outstanding findings

`docs/plans/175-pij-skill-realign/assets/survey/routes.md:9,36` names generic documented skill-installation locations, not personal files, session caches, transcripts or session identities. These are permitted by pm-001's clarification and need no edits.

The earlier root-prefix objection is withdrawn, not retained as a finding or note owed. The rule concerns personal or host-state information, not every generic path under a harness home directory.

## Validation

- Corrected session probe `review-116-scrub.py`, with `--repo` selecting `fleet-review-116b` and `--head 279b0ec430edcb0b64ed39f76afbbb6ac111be20`: exit 0, no violations, 20 positive/negative controls pass. These include permitted install locations and planted personal-state paths. Receipt: `review-116-fix-scrub.json`. It reads immutable blobs and emits only counts and locations, never the source identity.
- `ddocs validate docs/plans/175-pij-skill-realign/plan.dd.json --json`: exit 0, depth 3, zero errors and zero warnings.
- `ddocs build docs/plans/175-pij-skill-realign/plan.dd.json --check --json`: exit 0, `drift: false`, no writes.
- `git diff --check origin/main...279b0ec430edcb0b64ed39f76afbbb6ac111be20`: exit 0.
- `gh run view 41771832835 --json headSha,status,conclusion,jobs,url`: completed, success, exact reviewed SHA. `check (22)`, `check (24)`, `rust-lint` and `rust-test` all succeeded. Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/41771832835
- No loop, poll, timer, hook, sweep or process-launch behavior changes; no runtime cost delta.

`harness boot --json` returns exit 0 with `ready: false` because linked-worktree Vitest is absent. `harness checks --quick --json` returns exit 1 with six local tooling/configuration failures; native Copilot contracts pass, attribution is unavailable, Rust and smoke are skipped. These are not green local gates or additional PR regressions. Direct ambient ddocs validation and render checking above did pass.

No dependency installation/linking, Cargo command, product build, daemon change, product edit, commit or push was performed.

## Harness feedback

**magicWand:** one counts-only documentation scrub sensor with runtime-derived identities, source-derived path controls and paired permitted-install/personal-state examples. The reusable session probe now implements these controls and accepts an exact worktree/head pair.

**difficulties:** a blanket root-prefix check misclassified generic install locations. The corrected probe keeps those locations permitted while detecting planted personal-state paths. Local composite gates remain dependency-limited. The ignored observation buffer's earlier path-policy wording is superseded by this clarification.
