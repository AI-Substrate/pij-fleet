# Routing: send it, don't do it

| What | Send to |
|---|---|
| step packets, assignments, migration numbers, PRD state flips | pm/001 |
| `crates/daemon/src/delivery/**`, send and delivery paths, reply modes, the role queue, time-limit check-ins | delivery |
| `.dd/schemas/pij/**`, `docs/project/**`, doc procedures and examples, role templates | docs-system |
| a security verdict on every PR (exact head) | security |
| a cost verdict on every PR touching a loop, poll, timer, hook, sweep, status line or process launch | performance |
| correctness review of a PR | a reviewer, assigned by pm/001 |
| merges, rulings, the directory itself | prime |
