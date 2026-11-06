# PR #129 interim security finding

Source head: fd7bf4141420f132974ece403faa5ae8c64c9982.
Writer: pij-wary-lynx, security role, gpt-6.1-sol.

**Interim blocker, not final approval.** The requested posted verdict is held until the replacement merged head and its CI are checked. No PR comment is posted for this old head.

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| 1 | 🟡 MEDIUM | crates/daemon/src/delivery/role_send.rs | 585-644 | A stale assignment drain can deliver requests created after release to the released holder | 9/10 |

## Counterexample

1. Assign live seat A, scheduling its asynchronous drain.
2. Release A before that drain selects pending requests.
3. Another sender queues a new request for the now-unheld role.
4. The delayed hook selects that request and sends its body to captured seat A.

The drain does not revalidate the captured seat's current assignment. Delivery admission checks registry/tombstone state, not directory ownership; A remains live. Marking the request delivered after sending cannot prevent the disclosure.

This requires local directory-operation access and an asynchronous interleaving, not unauthenticated or paired-machine access. Any-seat directory management is intentional, but it does not make a released seat the current recipient of later requests.

## Evidence and repair

The security reviewer exercised real migrations 0032/0034 and the exact queue SELECT entirely in memory. After committing A's release, open holders were empty, yet the delayed-drain selection still returned the subsequently created request for the captured target. No real transport, production role send or forced wake was exercised.

Bind queue draining to the current assignment identity/generation and coordinate request claiming/delivery admission with release and reassignment. Do not rely on a post-send update or an uncoordinated stale snapshot. A deterministic regression should delay A's hook, release A, queue a later request, and prove A cannot receive or consume it; the valid later holder must receive it only once.

A merge-only replacement does not resolve this logic finding. Full remaining logic coverage, replacement-head verification and exact CI proof are still in progress; no broader pass or completed live-delivery reproduction is claimed.

## Completed logic handoff

The old-head logic review is now complete: merge-base 0eebb69ac0e0cf81dd3d98c322d3f0f7ab4308f7, 35 files, +2987/-21. No old-head CI exists. The new merged head da23eb7566844c93857ecb11ba58c4da6947c605 and its reported green CI are awaiting carryover verification.

In-memory Python/SQLite proof receipts 462 and 464 exited 0. Real migrations 0032/0034, a committed release and exact source SELECT returned the later request with zero open holders. These are SQL/interleaving checks, not Rust integration or live transport runs. No filesystem proof logs were created.

Local-bearer/asserted-from and shim caller derivation remain the existing trust models. Paired-machine sends refuse before role resolution; no new body-reading HTTP endpoint was found. Immediate resolution applies role/project/instance/live/mismatch guards, but the drain bypasses current ownership validation. No additional independently graded data-access vulnerability was established.

Related contract note: delivery/role_send.rs:637 directly admits non-FYI drained bodies without the cold guard/audit used by immediate sends at http/role_send.rs:144. Align that admission with the existing cold-wake contract while repairing the drain; no live cold wake or additional graded access exploit is claimed.

Zero-holder PA/prime nudges use fixed daemon identity, request-derived IDs and a fixed role-queue reason; the watchdog records mandatory cold-wake audit before sending and audit failure prevents the nudge. C4 requester-reply forced wake is not yet implemented in this head: requester/reply mode are stored for s06, so later answer ownership/consent/notification behavior is not certified.
