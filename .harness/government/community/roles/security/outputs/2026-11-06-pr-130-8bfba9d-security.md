# PR #130 security verdict

security: pass-with-notes (head 8bfba9de1206a0f781bd853827df38abc961ca08)

No exploitable vulnerabilities found in the reviewed changes. This carries the prior dcc3e52 security assessment and checks the final replay, recipient-key and sanitization repairs.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| - | - | - | - | No vulnerabilities found at this head | - |

## Final repairs and retained boundaries

F001/F002 are resolved: the compact-route FYI identity is `routes-after-compact:<seat>:<msg_id>`. Persistent FYI deduplication suppresses replay before and after claim, without collapsing another recipient's notification. Both regressions pass in CI.

Cells now discard ESC, BEL and the specified bidi controls. Both paths in seat-facing overlap warnings use cell(), preventing raw line-break/control-sequence insertion there. This closes the prior sanitizing note, not every possible Markdown interpretation.

The notification remains a held FYI with a fixed daemon sender, a selected recipient and command: None. Scope text does not select controls or shell commands. Existing authorization, holder-snapshot exclusion from the persisted routes file, writer confinement and protected-filesystem/same-user qualifications remain.

Source: [recipient-qualified replay identity](https://github.com/AI-Substrate/pij-fleet/blob/8bfba9de1206a0f781bd853827df38abc961ca08/crates/daemon/src/http/mod.rs#L2055-L2076), [cell filtering](https://github.com/AI-Substrate/pij-fleet/blob/8bfba9de1206a0f781bd853827df38abc961ca08/crates/daemon/src/community/routes.rs#L359-L371).

## Evidence and limits

Local and GitHub heads matched; the review worktree was clean. GitHub reports OPEN/CLEAN and four completed SUCCESS checks. [CI run 42149295542](https://github.com/AI-Substrate/pij-fleet/actions/runs/42149295542) has a synthetic-merge tree identical to the reviewed head, with 1,901 JUnit cases and zero failures/errors.

Scope rules still permit Markdown; comprehensive literal rendering and semantic prompt-injection immunity are not established. Compact notification remains admission-timed, not proof of completed compaction or universal first-post-compact placement. CI reports ddocs UNAVAILABLE, not document validation.

Source and existing CI evidence were used. No local build, feature invocation or production mutation was performed. Publication text was screened for credential patterns, private identities and prohibited machine-local paths before posting. This verdict covers only the full head above.
