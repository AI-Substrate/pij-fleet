# Approved skill wording: one build per PR (Jordan, 2026-11-11)

Approved as written; Jordan said "yes" on 2026-11-11 at about 11:59. The docs PR uses this text **verbatim**: no extra rules, no load numbers, no baton, no lane cap.

## 1. `skills/pij/references/00-routing.md`, C12

Rename it **C12 — Shared machine: heavy work is niced, capped and built once per PR**. Keep the existing per-run paragraph unchanged (nice -n 19, 8 workers, `cargo -j 4`, `timeout 600`, stop above load 40, flake reasoning), then add:

> **One build per PR.** Every PR has one **build owner**: the seat doing the PR's main work (its coder), or the workstream's main agent if it has no coder. The build owner builds the final head once and records where the build is and what it ran (tests, results) in the review packet. Reviewer, security, performance and every other seat **reuse that build**: they read the diff and the build owner's results, and run the build owner's binary if they need to try something. **They never build their own copy.** If a reviewer needs something run, it asks the build owner for that specific check. Where the project has CI, CI's run counts as the build.
>
> **Coders build what they touch** while working (`cargo test -p <crate>`). The full suite runs once, on the final head, by the build owner.
>
> **Close seats you're done with.** Memory first, then `pij close`, then kill the pane and check the process is gone. An idle seat still costs RAM and CPU.

## 2. Failure-mode rows (each route's existing `## Failure modes` table)

| Route file | Signal | Move |
|---|---|---|
| `routes/community.md` | More than one seat building the same PR | Stop. The build owner builds; the others reuse its build (C12). |
| `routes/pair.md` (add a Failure modes table if it has none) | More than one seat building the same PR | Stop. The build owner builds; the others reuse its build (C12). |
| `routes/prime.md` | The machine is slow, or the human says so | Stop new builds and lanes, close idle seats, make sure every PR has one build owner, then report. |

## 3. `skills/pij/references/prime/orient-oprime.md`, new duty 14 (after 13)

> 14. **Every PR has one build owner.** Name it in the review packet; reviewers reuse its build. Close seats you're done with.

## 4. `AGENTS.md` (pij repo), "Resource cost is a correctness property", new item 6

> 6. **One build per PR (skill C12):** the PR's build owner (CI here) builds once; reviewers reuse it and never build their own copy.

## 5. Guard

Add a marker per new failure-mode row and for the C12 "One build per PR" paragraph to `harness/scripts/pij-skill-check.sh`, so the text can't silently drop out. PR body: `Delivers: none`. Verdicts: reviewer and security (docs only, no performance).
