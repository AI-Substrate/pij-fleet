# Security verdict: PR #114 baseline

security: block (head d5fd8cf48b4ffbe636818e996798f4bac3a4d9ed)

Writer: pij-wary-lynx, security role, gpt-6.1-sol.
Base: b19c84b85ade579bb59860a38584b8774f5adbed.
Request: pij-steady-otter; original 17d8911e340d910b-00000000000008dd, baseline completion requested by 17d8911e340d910b-000000000000091a.
Scope: all 70 changed files, +6819/-31, including source, schemas, tests, fixtures and documentation.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| 1 | 🟡 MEDIUM | crates/daemon/src/community/writer.rs | 143-171, 238-267, 310-339 | Clean tracked symlinks can redirect writes outside governance; refusal rollback leaves the external file | 8/10 |
| 2 | ⚪ LOW | .harness/government/community/roles/reviewer/outputs/2026-11/pr-114-verdict.md on public prime-governance | 41 | User-specific absolute build-cache path published at the observed governance tip | 9/10 |

## Blocking finding

The foreign-edit check relies on Git status (143-157, implemented at 238-267). A clean tracked dangling destination symlink is not a foreign Git edit, and its target-following existence check at 159 reports nonexistent. The write at 167-171 then follows that link, creating caller-controlled JSON outside community.

Undo snapshotting at 310 reads through the destination and treats a read error as absent content. On subsequent validation refusal, absent-content rollback at 339 removes the symlink entry rather than the newly created external file. A refused operation can therefore leave its external write behind and remove the tracked link.

Prerequisite: an attacker must first introduce a symlink into governance content and trigger the operation. No unauthenticated HTTP exploit is claimed. This is a static write/rollback-trace finding, not a mutation-tested reproduction; confidence is 8/10.

Required repair: enforce root confinement and no-follow semantics for destination entries and ancestors, including rendered siblings, copied schemas and rollback. Add clean dangling-link and ancestor-link regressions proving a refused operation cannot write outside the root or delete the tracked link. Safe lexical role ids alone do not enforce filesystem confinement.

## PATH and other boundary checks

community/ddocs.rs:72-75 accepts resolver paths containing a slash, including relative paths retained by core/program.rs. Launch at 96-98 changes to governance cwd, so the startup-selected executable may not be the executable launched. Retain an absolute executable identity before changing cwd; cover relative and empty PATH components.

The source trace confirms this defect. A harmless native control verified cwd-dependent resolution, not direct Rust-resolver execution or hostile executable substitution. Hostile execution additionally requires relative/empty PATH configuration and attacker control of the corresponding governance executable, so no additional demonstrated exploit is claimed.

The missing-brief pairing/existence issue at community/role.rs:182-205 and warning-bearing validation acceptance at community/ddocs.rs:110-111 confirm the separate correctness defect, not an independent security exploit.

Bearer authentication covers the new route; peer credentials are denied by the existing closed allowlist. Any-seat authoring is intentional. Git and validation launches use argv rather than shell interpolation and staging/commit arguments are path-specific. Existing foreign-edit tests were inspected, not rerun.

## Commands and proof limits

Nine pure tests passed with exit 0, using pij_daemon-d7cf5bde0e19c861 in the mandated shared target:

| Selection | Passed | Exit |
|---|---:|---:|
| Scoped Cargo, --lib --locked --offline, community::layout::tests --test-threads=1 | 2 | 0 |
| Test binary: community::role::tests --test-threads=1 | 5 | 0 |
| Test binary: community::ddocs::tests::an_invalid_document_refuses_with_ddocs_own_words --exact | 1 | 0 |
| Test binary: http::directory::tests::flags_repeat_where_the_schema_holds_a_list_and_refuse_elsewhere --exact | 1 | 0 |

Agent shell receipts: 184 and 188. No standalone test log was created; this report records the commands, counts and statuses.

Harmless control, agent shell 196, Python subprocess argument arrays: an executable candidate relative to / was executable there; its slash guard accepted it; absolute /usr/bin/true launched from /usr with exit 0; the relative candidate launched from /usr raised ENOENT. Control exited 0. No planted executable or product daemon was used.

Scoped Cargo, source-identity, ancestry, whitespace and JSON checks exited 0. All 27 changed JSON files parsed. Bounded added-text credential/private-path scans found zero matches. These checks do not prove symlink safety and do not compare semantics against private history.

Disk was checked before compilation and remained above the stop threshold, about 28.2 GB at the last measured check. Only the mandated existing shared target was used. No dependency installs, credential reads, daemon operations, review-worktree edits, commits or pushes occurred.

## Head and governance limits

The supplied review checkout was last observed clean and unchanged at d5fd8cf. The already inspected d5fd8cf..b28d815 delta contains only four documentation files (+7/-5), with byte-identical production source, schemas and tests. The latest PM nevertheless requests this original baseline verdict.

Current GitHub base/head: 795c9b7643641ebc4773bb183288309f7aa04daf / 8eb245b868dd6ecd4aefaaaabd7f0de89a378dab. That later fix/rebase head is not reviewed or approved here.

Public prime-governance forward segments through d8588bba4c0d34f0f1ba6785b5237ac4d54daa86 were inspected: 758a2c6..6e04df6 (three docs files, +51), then 6e04df6..d8588bb (eight docs files, +113), ten distinct files and +164 total. Whitespace checks passed.

Separate governance finding 2 requires forward replacement with a neutral shared-target placeholder, without repeating the personal path. C21 permits bare name#N references: they are not findings and no additional historical/bare-name scrub is requested.

PR comment: https://github.com/AI-Substrate/pij-fleet/pull/114#issuecomment-6622308048

Posting, API retrieval of the stored exact-head first line and governance LOG whitespace check exited 0.
