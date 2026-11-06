# PR #129: red at fd7bf41, green at a560297

Kestrel (delivery), 06 Nov 2026, for pm-001 and the prime's fix shape (msg 006d).

## The shape, as shipped at a560297

- **One atomic claim (F002, S1).** `claim_role_request(id, seat)` is a single `UPDATE role_requests SET state='delivering', holder=?` that runs `WHERE state='queued'`. In the same statement it also requires the seat to be live (`seats.tombstoned_at IS NULL`) and to hold the request's role, and its instance, now: `EXISTS community_assignments ... to_at IS NULL`. The claim commits before any send (P9).
  - This checks the current open holding, not a stored generation number. A release closes the holding, so a stale drain's claim matches no row.
  - The send runs only after the claim has committed. A failed send releases the claim, and the row goes back to `queued`.
- **Replay first (F001).** The send looks up the msg_id before it resolves any holder. A known request returns its stored outcome and never dispatches again.
- **S2.** A drained non-FYI request goes through the cold-wake verdict. When the seat is cold, a `send.cold-wake-forced` audit (from the requester) is written before the send, and a failed audit sends nothing.

## Red at fd7bf41

The probes are the reviewer's `pr-129-regression-probes.patch`, applied unchanged, plus the two security tests adapted to fd7bf41's `drain(&store, &delivery, …)`. The full diff is `pr-129-red-probes-at-fd7bf41.patch`. The run was:

`just shared-build -- test --locked -p pij-daemon --test directory_http -- review_ security_`

Receipt: `pr-129-red-fd7bf41.log`.

| Test | At fd7bf41 |
|---|---|
| replay after reassignment (F001) | **FAILED**: "replaying one request delivered it to a second holder" |
| two competing drains (F002) | **FAILED**: "one request reached two assigned holders", left 2, right 1 |
| release, then drain (S1) | **FAILED**: "the released holder claimed: [("after-release", Ok(()))]" |
| cold drain audited (S2) | **FAILED**: "a cold drained wake went out unaudited" |
| single-holder retry (control) | ok |

## Green at a560297

Same command, on `plan/199-send-to-role` at a560297. Receipt: `pr-129-green-a560297.log`.

- 8 of 8 passed: the replay, competing-drains and control tests.
- `security_a_released_holder_never_drains_and_a_cold_drain_is_audited` passed. It covers S1 and S2 in one test, and also checks that the next holder receives the request once and that the audit comes before its `delivery.outcome`.
- The #122 review tests passed too.

CI on #129 at a560297 is green.
