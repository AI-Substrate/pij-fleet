# Plan 181 rulings (prime, 2026-10-28)

- **R1 Base.** Branch `fix/codex-bind-identity` from main 26fc3ba. Do not touch `crates/tmux` or the pane-capture path: PR #73 (plan 180) rewrites it. Keep diffs in `registration.rs` / `http/identity.rs` minimal and name the overlap with #73 in the PR body.
- **R2 Diagnose before fixing.** Jordan: "this seems like a new thing though so be careful", and plan 177 changed Codex parsing four days earlier. Every fix cites a cause reproduced in `assets/evidence/diagnosis.md`. The `⚠ 5h limit … · /status` misparse (ac-0004) is true on its own but is NOT assumed to be the bind cause. If a cause lies in Codex itself, stop and report to the prime.
- **R3 Canaries.** Reproduce with real `pij-rs spawn --harness codex` seats on the machine daemon, as your own children: at most 6, each in its own window (spawn's default). Close each one (`pij close`), kill its window, and list them in diagnosis.md. Never bounce, restart or reconfigure the machine daemon. Never run a second daemon: it would tap every live pane. Never send to or touch any seat you didn't spawn (the osk seats included). Reading the store, events and logs is fine.
- **R4 osk.** Read the osk report only; change nothing under ~/osk.
- **R5 Builds.** Build normally; there is no load gating. Wrap Codex CLI calls in `timeout -k`.
- **R6 Quota.** Pick the canary model and effort with `pij models`; use the cheapest effort that reproduces the defect.
- **R7 Ship.** Commit, push the branch and open the PR with `Fixes #76` (standing approval). Do not merge. After every push, `git ls-remote origin 'refs/notes/*'` must be empty.
