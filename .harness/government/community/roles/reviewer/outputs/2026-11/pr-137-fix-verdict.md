# PR #137 repair verdict

**pass-with-notes** at `1fbe46c4ab695d0f43b3e8e65b189fa68ac8fb26`. F001, F002 and N001 are closed. No correctness changes owed.

## Scope

Reviewed only the delta from `4ab6460`: repairs `def622a49f1b3ede951cdc1085a6dd91c3ac3dc8` and `cb9aa1427a9239d40c8a6f5314502788b6c88087`, plus the main-merge composition. The detached `~/pi-hacking/review-137-1fbe46c` worktree is clean and pinned. GitHub reports main as the base, CLEAN, and the same exact head.

The original s06 assessments otherwise stand. Main's independently reviewed features are not re-reviewed wholesale. Security's S001 and forwarding verdict, and performance's cost verdict, remain separately owned.

## Finding closure

**F001:** migration 0036 now declares `PRIMARY KEY (project, path)`, with both columns non-null. `record_output` uses the matching conflict target. Both role and subject queries retain project filtering and exact-head equality. The store regression inserts the same relative path under two projects, reads the second back and verifies the first is unchanged. The amendment is to the still-unmerged migration, not a rewrite of a deployed production migration.

**F002:** notification identity is `output-<project>-<commit>`, passed from the writer's actual commit into both the message and response. It no longer depends on a clock reading. Each output occupies its own writer commit; the concurrency regression checks distinct commit-derived IDs and two held FYIs, not merely two files. The existing per-message dedupe behavior remains applicable to a replay of the same notice.

**N001:** upper-seat lookup now returns `Result<Option<SeatId>>`, propagating registry/store failures rather than treating them as absence. A forced-wake guard error is decoded from its response envelope and reported with status/cause in `notified.outcome`; that branch never calls delivery. The guard's adapter failures carry their message in `meta`, which the new decoder reads. A genuinely absent seat remains a distinct result. This closure is source/dataflow proof, not a newly injected cold-audit/registry failure.

## Merge and command composition

At `317ebd8`, `delivery/role_send.rs`, store role-request code, the PA watchdog and migration 0035 exactly match the merge's main parent. Main's directory HTTP tests and PA digest source/tests match too. The output additions coexist with memory/claims/delivery: `Endpoint::ALL` has 58 entries, both output handlers are registered, the output module/export remains, and schema version is 36.

The last merge brings main's #130 routing-table implementation, not literally only documentation. Its s06 output implementation, migration, tests and native-verb manifest are unchanged from `cb9aa14`; the shared router retains both output endpoints alongside main's routes module/control hook.

`output` and `outputs` are now in the native manifest and matching TS route union/table. The clap equality guard and generic full-shim tests cover the expanded list: help equivalence, flags/argv/stdin preserved, including `--body-file -`. No new selector or body re-parser was introduced.

The answer-authority check reads the stored request and precedes writer, index, answer stamp and notice. CI covers refusal without those effects, open publishing without `--answers`, and recorded-holder/live-holder/project-prime cases. The new refusal is mapped to HTTP 403. These ordering checks support correctness; they are not a substitute for security's independently assigned verdict.

## Exact-head evidence

[CI run 42224735209](https://github.com/AI-Substrate/pij-fleet/actions/runs/42224735209) completed successfully in all four jobs. Rust ran **2,021 passed, five skipped**. Downloaded JUnit confirms:

- all six `output_http` tests, including two-notice concurrency, answer authority, reply modes, non-holder publishing and exact-head reads;
- `outputs_answer_for_their_exact_head_and_list_by_role`, including the two-project same-path assertions;
- the clap/native-manifest equality and value-flag-table guards;
- all six full-shim tests, including `every_community_verb_and_flag_reaches_pij_rs_unchanged`.

The shim fixture iterates the manifest, so the two new verbs participate rather than relying on a copied four-verb list. CI logs and JUnit are one CI origin. `git diff --check 4ab6460 HEAD` passed. No local builds, test suites, daemons or probes ran.

## Notes and limits

No live cold requester or new audit-failure injection is claimed. The reported writer-lock retry flake did not fail this exact-head run; no unrelated fix or historical trailer rewrite is requested. The known old commit-trailer warning remains acknowledged.

The delta adds no timer, poll or per-seat/per-tick launch. Commit-bound ID formatting is in-process; forwarding reuses the established one-exec path. Live cost acceptance remains performance-owned.

PM request: `01a1b10b-c4b4-7763-9108-cb9340910030`. The live routing table was read as requested. No product files were changed.

**Publication:** [pinned passing comment](https://github.com/AI-Substrate/pij-fleet/pull/137#issuecomment-6686219808) verified with F001/F002/N001 closure present. Its generated receipt was appended once to `roles/reviewer/outputs/LOG.md:139`. Role memory was written through the verb in governance commit `b45d20a`.
