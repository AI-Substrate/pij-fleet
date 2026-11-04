reviewer: block (head 75102c5a16060632d4fe325dd519ab54f5e06cfb)

Scoped re-review of PR #111 from b13f5b3 to the frozen replacement head, in fleet-review-111b. Both original P2 findings are resolved. One blocker remains in the newly changed triage; unchanged portions of the prior review stand. No product files changed.

## One blocking finding

**F003 / P2: The newly dropped pane-delivery rows lose their live-proof requirement.** In `docs/plans/192-prd-register-move/assets/evidence/triage.md`, old req-0005 and req-0022 are now `dropped-met`, citing `crates/tmux/src/lib.rs:413-443` and `972-977`. Those locations prove that an implementation chunks pasted text and rejects unsupported controls. They do not prove that the real harness composer accepts and submits the complete message. The two rows include live per-harness delivery and large-body witnesses, not just a generic paste implementation. No such receipt is cited; pm-001 confirmed no known witness pointer. Removing the remaining proof need from PRD 002 is therefore unsupported, unlike the consistent treatment of Claude's remaining live-proof gap in req-01.

**Concrete single-push fix:** retain the existing implementation as met, but change both old ids to `kept` and map them to one additional PRD 002 row, preferably req-14 so the current ids need not move. Keep it narrowly about **live pane-delivery proof**, should/planned: demonstrate the socket-unavailable route on real Codex, Copilot and Claude composers, plus an OMP witness for large-body delivery. Its done_when should require recorded receiver-side evidence that a 3,000-byte body arrives intact and reaches an agent turn on the first attempt, not merely a successful paste call. Preserve the existing queue/refusal guarantees without redesigning or reimplementing the transport. Update the triage, plan/roadmap counts, scrub output and generated faces together. With one combined new row, the accounting becomes **16 kept as 14 PRD rows, 19 met, 5 obsolete**. This PR need not perform those future live runs; it must retain the unmet proof work rather than mark it complete.

## Closed findings and sampled receipts

| Subject | Result |
|---|---|
| Original typing-gate P2 | Closed: old req-0004/0030 now dropped-met with the actual expiry, self-injection and fresh-boundary implementations/tests in `crates/harnesses/src/interaction.rs`. |
| Original Claude-default P2 | Closed: new req-01 acknowledges default acceptance and retains only live hold/deny proof. `crates/transport/src/uds.rs:51-54` explicitly leaves that connection shape unproven; tests at 1459-1593 use a local stand-in socket. The obsolete default question is gone. |
| Kept req-01 / old req-0002 | Supported by the UDS disclaimer and stand-in tests above, rather than a claim that the socket implementation is absent. |
| Kept req-02 / old req-0006 | Supported: `crates/cli/src/main.rs:324-341` exposes revive arguments but no preview. |
| Kept req-03 / old req-0012 | Supported: `crates/cli/src/main.rs:274-323` exposes cwd and no first-task field. |
| Kept req-05 / old req-0026 | Supported: `crates/harnesses/src/spawn.rs:142-146` constructs child identity variables without the daemon address/state directory. |
| Newly met old req-0008 | Supported: registration clones the existing descriptor and only fills absent Claude consent (`crates/daemon/src/registration.rs:850-884`); test at 3467 checks unchanged attested ownership and binding. |
| Newly met old req-0003 | Supported: `crates/daemon/src/reaper_tests.rs:293-335` proves unknown or still-live processes cancel retirement, including the no-pane-probe case. |
| Newly met old req-0018 | Supported: `crates/daemon/src/death_sweep.rs:81-95` invokes canonical reconciliation; daemon wiring at `lib.rs:1048` starts it. `reaper_tests.rs:338-374` verifies one committed tombstone, and `delivery/mod.rs:4306-4326` refuses later delivery. |

All 13 current PRD rows have corresponding evidence entries, all remain planned, and req-13 preserves C17's could priority and no-unearned-binding constraint. The new security-related edits remove the literal source identity from the plan question and learn it only at checker runtime. Security's exact-head verdict remains separate.

## Validation

| Command/check | Result |
|---|---|
| `python3 docs/plans/192-prd-register-move/assets/evidence/check.py <private-checkout>` | Passed: 40/40, 14 kept, 21 met, 5 obsolete, 13 targets; no accounting problems, scrub hits or shared eight-word runs. |
| Same checker with `--self-test` | Passed on the synthetic identity. |
| Reviewer scratch mutation probe | All six controls passed: clean, copied eight-word run, missing verdict, duplicate verdict, planted runtime-derived source identity, restored clean. The four negative controls exit 1; clean controls exit 0. No source text/identity was printed; scratch copies removed. |
| `ddocs validate docs/project/prds/002-pij-base.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs validate docs/project/roadmaps/001-pij-community.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs build <file> --check` on the revised PRD, roadmap and plan | All three passed with no drift or writes. |
| Main-checkout `node_modules/.bin/biome check docs/project/prds/002-pij-base.dd.json docs/project/roadmaps/001-pij-community.dd.json` | Passed, two files, no fixes. |
| `harness boot --json` | Advisory not-ready: no Node dependencies in this review worktree; no install or linking performed. |
| `harness checks --quick --json` | Same tooling limitation: five failed local stages, native Copilot contracts pass, Rust/smoke skipped. Not a green local full gate. |
| `gh run watch 41767775916 --exit-status --interval 20` and exact-head read-back | Passed: check (22), check (24), rust-lint and rust-test all successful at 75102c5a16060632d4fe325dd519ab54f5e06cfb. |

The checker now correctly states that it proves accounting and lexical checks, not semantic verdicts. There is no changed product loop or recurring process-launch cost.

Minor note, not a blocker: triage's introductory “seven rows” refers to the seven removed draft PRD entries; nine source-table rows actually changed from kept to met. Clarify that noun while updating the counts, if convenient.

magicWand / captured difficulty: a triage receipt should name its proof grade, so implementation evidence cannot silently discharge a live-witness obligation. The counts-only mutation harness is preserved as a session artifact; no additional product work is requested by this review.
