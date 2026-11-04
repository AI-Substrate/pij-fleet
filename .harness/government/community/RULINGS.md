# Community rulings (prime, within Jordan's rulings in PRD 001 and workshops 001–002)

| # | Date | Question (from) | Ruling |
|---|---|---|---|
| C1 | 2026-11-04 | `role:pa`/`role:pm`: directory assignment or seat_roles when they disagree? (delivery) | The directory is the authority for `role:` addressing. s04's assign also sets seat_roles for pm/pa/worker, so they agree. If they still disagree, the send refuses with a named mismatch rather than guessing. |
| C2 | 2026-11-04 | `--fyi` send to a role with 0 holders: refuse or queue? (delivery) | Queue, like any send to a role with no holder, and wake the PA (else the prime). The FYI flag governs how the eventual holder gets it, not whether it queues. |
| C3 | 2026-11-04 | How does an output link to its request? (delivery) | `pij output … --answers <msg_id>`; the role-send receipt prints the request id. Owner: c-verbs (s06). |
| C4 | 2026-11-04 | `--reply wake` to a cold requester? (delivery) | An audited forced wake, as the PA watchdog does. The requester asked to be woken; falling back to an FYI would strand it. |
| C5 | 2026-11-04 | Where does req-22's default limit live? (delivery) | A `default_limit` field in the role schema, added in s02. |
| C6 | 2026-11-04 | Draining the role queue on assign (delivery) | s04's assign exposes a hook that s05 uses to drain the role's queue to the new holder. |
