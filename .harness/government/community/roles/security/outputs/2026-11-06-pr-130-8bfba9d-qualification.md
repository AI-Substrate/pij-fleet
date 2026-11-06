# PR #130 final-delta qualification

security: pass-with-notes (head 8bfba9de1206a0f781bd853827df38abc961ca08)

This supplements, without overwriting, the existing 2026-11-06-pr-130-8bfba9d-security.md and verified PR comment https://github.com/AI-Substrate/pij-fleet/pull/130#issuecomment-6676132545. Both predate the queued review request. No duplicate canonical verdict comment was posted.

The requested 9d2a746..8bfba9d delta was independently read: two commits, five files, +92/-11. The prior dcc3e52 assessment and 9d2a746 source notes were retained. No new exploitable security vulnerability was established.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| - | - | - | - | No new exploitable security vulnerability established | - |

## Nonblocking custom-ID correctness qualification

http/mod.rs:2055-2076 uses routes-after-compact:<seat>:<msg_id>. This separates the ordinary generated seat IDs in the new regression, but the encoding is not unambiguous over every admitted string.

For example, seat pij-a with message ID b:c and seat pij-a:b with message ID c both produce routes-after-compact:pij-a:b:c. These are illustrative identifiers, not runtime observations.

SeatId is a transparent string at crates/core/src/model.rs:11-32. The traced registration check at crates/core/src/admission.rs:125-153 requires nonempty identity rather than excluding colons; the message-ID check at http/mod.rs:1848-1859 excludes @, not colons.

This is a custom-ID/F002 correctness edge, not an established disclosure, authorization bypass or execution vulnerability. No collision probe was run and no additional code repair is demanded by this security verdict. An unconditional partitioning claim needs unambiguous tuple encoding or an enforced delimiter-free seat grammar; the separate correctness owner decides its disposition.

## Verified protections and limits

community/routes.rs:346-373 normalizes whitespace, removes char::is_control characters and the specified U+202A-202E/U+2066-2069 ranges, then escapes pipes. It reaches persisted paths/rules/overlaps, seat-facing rows/holders and prime change-row text. Seat-facing overlap warnings apply cell() to both paths at 56-68.

This is not complete HTML/Markdown literalization or removal of every Unicode format/bidi character. Structured source scope data is not rewritten. Existing authorized-governance-author, holder-free persisted file, writer, local-origin, selected-recipient, command-None and held-not-waking boundaries remain.

Deduplication survives the delivered transition while the row/database is retained. The third-attempt test directly updates state; it is not real claim-API integration proof. Compact notifications remain admission-timed, not confirmed-completion proof.

## CI and publication state

Run 42149295542 attempt 1 has four successful jobs. Checkout merge 7fc8d5f47947fa44a27da0281a8d14e1da3afa1f has tree 31f6d9d87d71d30e8ca08291998df1a330324cde, equal to the reviewed head. JUnit artifact 13471936369 contains 1901 cases, zero failures/errors; nextest separately reports five skipped. Both replay cases and sanitizer/overlap cases pass. Broader character coverage is source-predicate proof, not an individual test for every character. ddocs UNAVAILABLE remains a validation limit.

The current head still matched the pin, but the latest review metadata reported mergeable:false/dirty. Security approval does not clear merge conflicts or any replacement head. The existing report's earlier CLEAN observation is historical, not a current merge clearance.

The existing canonical comment was read back in full. The exact verdict line was explicitly re-sent to PM in native message d7bbd872-7cf4-4b77-bf99-dcb140f41d44. No existing report/comment was overwritten. No local builds, tests, probes, scripts, feature calls or production operations were performed.
