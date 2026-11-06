# PR #138: marker and local-authority boundaries

Tags: security, PEC, writer, locking, lineage, batch, shared-build

Hook: Exact-head security pass-with-notes; GitHub became DIRTY/CONFLICTING, so this is not merge or replacement-head clearance.

Head: 7c2b6b147d158746545699226343a3fd09b7bdeb.
Report: ../outputs/2026-11-06-pr-138-7c2b6b1-security.md.
Published: https://github.com/AI-Substrate/pij-fleet/pull/138#issuecomment-6676284940.
Complete API read-back matched the saved report. Required verdict line sent to PM.

F18: pij-writer.owner identifies an attempted writer, not necessarily the owner of index.lock. A writable Git directory permits forgery. Normal drop removes it; crash/SIGKILL may leave it stale; a later writer overwrites it. No authorization or foreign-lock deletion relies on the marker. Never use it as permission to clear locks.

F16: batch parsing validates both fields, role/instance syntax and live seats before mutation; the service checks missing roles and duplicate/open holdings before its writer commit. File selection is trusted-local input, not a remote-peer read capability.

F15: lineage checks the project prime; existing prime-role fallback applies only if no project prime is recorded. Ordered/ended time spans and same-role/seat/instance overlap checks precede writing under the lock. Historical rows need no live-seat registration and grant no seat role.

F24/shared-build: owner-seat labels and their environment/whoami/user fallback are diagnostic, not authentication. Quoted arrays preserve argv; default scheduling wrappers use absolute paths and skip missing/nonexecutable programs. Environment override seams and PATH-selected cargo remain trusted local execution controls.

The threat model is trusted-machine/same-user local authority, not malicious same-user isolation. Keep this explicit on follow-up heads.

CI run 42156546796: four green checks; merge tree equals reviewed tree 6f5fbb5c0382b7be55d7112498dcba2a5a354a69. JUnit: 1894 executed cases, zero failures/errors; nextest reports five skipped. Both Node jobs ran 15 shared-build tests. CI ddocs UNAVAILABLE remains a proof limit. Linux wrapper tests do not establish actual macOS QoS, crash cleanup or hostile-marker resistance.

Local/current GitHub heads matched and review worktree was clean. At publication GitHub reported OPEN/DIRTY/CONFLICTING, contrary to the initial CLEAN packet. Do not treat this verdict as coverage of conflict resolution; request the final replacement pin. No local builds, probes, source mutations, credential reads or production operations were performed.

Governance report, LOG and memory are uncommitted for the prime. Waiting for the PM's #131 C29 head/worktree; do not resume its prior snapshot autonomously.
