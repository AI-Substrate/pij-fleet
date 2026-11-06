# Plan 207 evidence: in-process UTC offset and registration ancestry

Phase `ph-00cf`, written by pij-following-marsupial (coder), 2026-11-06. The spike is in [spike-procargs.md](spike-procargs.md). Private-daemon script: [private-run.sh](private-run.sh); transcript: [private-run.txt](private-run.txt).

## What changed

- **#127.** `pij_harnesses::proc::local_utc_offset_minutes` reads the offset through `time::UtcOffset::local_offset_at` (libc `localtime_r`, `refresh_tz` on macOS), the ADR 0001 path, instead of launching `/bin/date +%z`. Every caller now launches nothing: claude_bind's per-pending-seat 5 s tick, FYI claim and read, carrier flush, Claude adopt, revive and the double-seat check. No new dependency.
- **#128, ruling C27 (option B).** Registration's ancestry walk reads each ancestor's parent and kernel name in-process (`proc_pidinfo` short BSD info on macOS; Linux already read `/proc`). It launches `/bin/ps` for argv only when the name is a harness host's (copilot, claude, codex, node). The cost is at most one `ps` per registration, down from one per ancestor. `unsafe_code = "forbid"` is unchanged. No new dependency.
- **Known difference:** the short BSD info keeps 15 of `p_comm`'s 16 characters. Registration compares names only with the four host names, so its results do not change.

## Cost (bp-0005), private daemon pid 72232

| Window | Launches |
|---|---|
| 60 s at rest | 0 |
| 3 × `pij-rs fyi-read` (each reads the UTC offset) | 0 (`date.utc-offset` absent) |
| paneless registration 1, real chain `sleep` → `node …/@anthropic-ai/claude-code/cli.js --session-id …` | 1 (`ps.registration-native-process`) |
| paneless registration 2, same chain | 1 |

The two launches present from the first reading (`ps.sidecar-process-group`, `tmux.control-client`) are daemon start, unchanged throughout. The live before and after pair (AC4) is the performance seat's.

## Tests (bp-0001 to bp-0003)

- `proc::tests::utc_offset_is_read_in_process_at_each_instant`: Melbourne +11 before 5 April 2026 and +10 in July, in a child process with an extra thread, with no launch. `utc_offset_matches_date` checks parity with `date +%z`. `path_search::utc_offset_runs_no_date_at_all` checks that no `date` decoy runs.
- `proc::tests::parent_and_name_match_ps_without_a_launch`: pid 1 (root), this process, and a reaped pid.
- `registration::tests::paneless_registration_reads_argv_only_for_the_host_named_ancestor`: claude, copilot and codex, native and node, each one argv read. `every_host_name_a_matcher_accepts_reads_argv` checks the gate covers every name a matcher accepts. `a_real_ancestry_matches_ps_and_launches_only_for_host_names` (macOS, alone in a child process) checks that the launch count equals the number of host-named ancestors.
- `spawn_sites::retired_launches_stay_retired` and `registration_ps_runs_only_behind_the_host_name_gate`, both mutation-checked. Re-adding `record("date.utc-offset")` plus `Command::new("/bin/date")` to the offset path failed the first. Making `read_native_process` call `read_argv_with_ps` directly failed the second. Both pass once restored.
- `first_light` now proves the spawn counter with the liveness `ps` fallback, since the offset no longer launches.

## Gates

- Local, before the fleet backoff: fmt check, `cargo metadata --locked`, and clippy `-D warnings` on pij-harnesses, pij-testkit and pij-daemon with all targets passed. Targeted tests passed: `pij-harnesses --lib proc::` 24, `path_search` 3, `pij-daemon --test first_light` 8, `pij-daemon --lib registration::` 89, `pij-testkit --test spawn_sites` 6, `store_wedge` 4. A wider run reached 34 binaries with 974 passed and 0 failed before the backoff hold stopped it.
- Under the prime's backoff (one build fleet-wide), the full scoped suite runs in CI on the PR head.
- `ddocs-check`, `local-path-check`, `lockfile-allowlist` and `pij-commit-trailers`, run with the main checkout's `tsx`: pass.

## Review round 1 (pij-pleased-marmot, block at d85c0f3)

- **F001 (P2):** the first cut read argv once per host-named ancestor, not once per registration. A `copilot` host under a `node` launcher, or a paneless chain with two `node` processes, launched two. Now one registration-scoped `BatchedAncestry` reads parents and names in-process. The first time an ancestor is host-named, it walks the chain above it in-process (to the 32-process cap) and reads every host-named pid's argv in one `/bin/ps -ww -p a,b,… -o pid= -o ppid= -o ucomm= -o args=`. Later observations answer from that read. A pid `ps` no longer finds reads as disappeared, as before. The matchers and identity checks are untouched.
- **Tests:**
  - `registration::tests::paneless_chain_with_two_host_named_ancestors_launches_one_ps` (`pij` → `node` helper → `node` Claude host) and `native_chain_with_a_node_launcher_launches_one_ps` (`copilot` → `node` launcher → pane `zsh`). Both accept the same host as before, and each records exactly 1 on the real spawn counter, run alone in a child process. The batch asked for is `[helper, host]` and `[host, launcher]`.
  - `a_batched_ps_reply_maps_back_to_each_pid` covers the parser.
  - On macOS, `a_real_ancestry_matches_ps_with_at_most_one_launch` checks this test's real ancestry: at most 1 launch, and each ancestor matches `ps`.
- **N001 (P3):** the tripwire is now `registration_gate_findings` over the source lines, with two negative controls run in CI. Replacing `native_observer`'s `BatchedAncestry::new(` with a direct `read_argv_batch_with_ps` call must be caught, and so must the reviewer's direct-return body. Both are.
- **Local proof (one build through the fleet-190 lock):** clippy `-D warnings` on pij-daemon and pij-testkit with all targets (macOS, where the batched path compiles), `pij-daemon --lib registration::` 92 passed, and `pij-testkit --test spawn_sites` 6 passed. CI on the head is the Linux proof.
