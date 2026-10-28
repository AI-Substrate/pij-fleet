# Plan 182 rulings (prime, 2026-10-28)

- **R1 Same semantics.** Liveness answers must be identical to `/bin/ps -o lstart=` today. No TTL, no cache (plan 180 R8: "a TTL that outlives a death changes delivery/whoami/reaper semantics"). If the precision differs between mechanisms, compare at the precision the identity check actually uses, and prove it on both OSes.
- **R2 Dependency.** `unsafe_code` stays forbidden in the workspace. Linux uses std only (`/proc`). For macOS, pick the smallest vetted crate, recorded in an ADR: permissive licence, actively maintained, minimal transitive deps, no network or codegen in build scripts, and a chosen version released at least 7 days ago (the same quarantine as `.npmrc`). Pin it; add the arch-allowlist row. If no candidate passes, stop and tell the prime.
- **R3 Resolve-once.** Keep hits only. If a kept path has vanished when launched, resolve again once. Never hardcode a Homebrew path.
- **R4 Measure.** Take before-readings on the live machine daemon (read-only: `pij-rs --json ping`, `just spawn-rate`). Never bounce, restart or deploy the daemon; the prime deploys after merge and takes the after-reading.
- **R5 Ship.** Commit, push the branch, open the PR with `Fixes #74` (standing approval). Do not merge. `git ls-remote origin 'refs/notes/*'` must be empty after every push.
