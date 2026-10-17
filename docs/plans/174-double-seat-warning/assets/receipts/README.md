# Plan 174 receipts

All commands ran from the plan's isolated worktree. Cargo used its local
`target`, `nice -n 19`, a bounded `timeout -k 10`, and `-j 1` for builds.
The retained logs are raw observations, not regenerated success summaries.
The first script red run was launched above the packet's load ceiling; the
final base-script red run was load-gated. See the retro.

## Red to green

| Check | Red on `bb9f0b1` implementation | Quoted failure | Restored implementation |
|---|---|---|---|
| Adopt code/details | `red-vd-0001.txt` | `adopt must lead with E-RS-CONVERSATION-LIVE, got 400` | `green-vd-0001-final.txt`: 7 passed |
| Pane-resolved verb matrix | `red-vd-0001.txt` | `/v1/whoami must lead with E-RS-CONVERSATION-LIVE, got 400` | Same daemon green log |
| Notice/restart, unreadable spine, incident | `red-vd-0001.txt` | `adopt must lead with E-RS-CONVERSATION-LIVE, got 400` (incident: `adopt from %7`) | Same daemon green log |
| SessionStart and prompt warnings | `red-vd-0002-final-tests-on-base-scripts.txt` | `hook output must be JSON (EOF while parsing a value at line 1 column 0): ""` | `green-vd-0002-final.txt`: 6 passed |
| whoami status line | Same script red log | Render contains `registering`, not the asserted red double segment | Same script green log |
| Heal refusal memory | Same script red log | `the refusal is remembered` | Same script green log |

The daemon red run has **5 failed, 2 passed**. The two passes are regression
pins for behaviour the plan keeps unchanged, so each is recorded as **pin, green
on base**, never as red:

| Pin | Guards | Status on `bb9f0b1` | Proof it can fail |
|---|---|---|---|
| `double_seat_without_a_matching_record_keeps_the_not_registered_text` | rk-0002: no record, or a stale record whose start does not match, keeps today's not-registered text byte for byte | pin, green on base | M5a, M5b below |
| `double_seat_a_resolved_pane_never_reads_records_or_probes_them` | rk-0001: a pane that resolves to a seat never reads process records or probes their pids | pin, green on base | M6 below |

Parent ruling (msg `c08e5737f7951d3e-0000000000000078`) accepted pins on that
condition: a pin that has never failed proves nothing, so each has a mutation
receipt. The script red run has **4 failed, 2 passed**; those two passing tests
predate this change. The initial `green-vd-0001.txt` is an intermediate **failed**
run (6 passed, 1 failed), retained for provenance, not cited as final green.

`green-vd-0001-plus-neighbours.txt` also records 132 passing daemon tests across
the `double_seat`, `revive`, `identity`, `fyi`, `adopt`, `whoami`, and `retired`
filters. These are fixture tests, not a live Claude or production-daemon smoke.

## Mutations

Each log includes the mutation, exit 101 and restoration of the original file.
The two final green logs above were run again after all restorations.

| Mutation | Receipt | Failure witness |
|---|---|---|
| Remove stray-record lookup | `mutation-m1-no-stray-record-lookup.txt` | 3 failures; whoami/send return the old not-registered refusal |
| Remove notice dedupe | `mutation-m2-no-dedupe.txt` | 3 failures; repeated notice count `left: 9`, `right: 1`; unreadable-spine brake fails |
| Ignore SessionStart code | `mutation-m3-sessionstart-ignores-code.txt` | 1 failure; warning stdout is empty |
| Ignore status-line code | `mutation-m4-statusline-ignores-code.txt` | 2 failures; rendered output says registering instead of double |
| Pin: alter the not-registered text (`identity.rs`, `owns pane` → `holds pane`) | `mutation-m5a-not-registered-text.txt` | 1 failure, the not-registered pin only; `left:` holds-pane text, `right:` owns-pane text |
| Pin: the choosing branch accepts a stale record (`double_seat.rs`, start check `.unwrap_or(false) \|\| true`) | `mutation-m5b-mismatched-start-verifies.txt` | 1 failure, the not-registered pin only; `mismatched start /v1/whoami` returns the coded 409, `left: 409`, `right: 400` |
| Pin: the resolved path reads records (`identity.rs` `resolve_seat` calls `verified_claude_record(state, \|_\| true)`) | `mutation-m6-resolved-pane-reads-records.txt` | 1 failure, the resolved-pane pin only; `a resolved pane probed a record's pid: [3999951, 3999971]` (`3999951` is the live seat's own pid, probed normally; `3999971` is the stray record's pid, which the pin forbids) |

After M5a, M5b and M6 were restored, `source-sha256.txt` verified every source
file and `green-vd-0001-after-pin-mutations.txt` reran the daemon tests: 7 passed.

## Other evidence

`vd-0003-clippy.txt` records the scoped daemon/harness clippy command with
warnings denied, exit 0. `gate-rust.txt` records `just rust-check` (workspace
`fmt --check`, `clippy --workspace --all-targets -D warnings`, `cargo test`),
exit 0: 1539 passed, 0 failed, 8 ignored. The TS stages (typecheck, vitest,
biome) are **NOT RUN** locally: this diff has no TS or JS, and the worktree has
no `node_modules` by rule. GitHub CI check (22)/(24) on the exact head is the
proof for those stages. The live-tmux smoke is **NOT RUN** (packet restriction).
`ci-pr56-80b8aba.txt` records GitHub Actions run 38675004257 on PR #56 at head
`80b8aba`: rust-test, rust-lint, check (22) and check (24) all succeeded, and
`git ls-remote origin 'refs/notes/*'` was empty. `resume-boot.txt` records a **not-ready** dependency
probe, not a green gate. Full gate, plan validation and CI evidence are recorded
separately in the execution log when observed.
