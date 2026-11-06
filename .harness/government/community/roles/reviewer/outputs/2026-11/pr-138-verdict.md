# PR #138 correctness review

## A. Verdict

**pass-with-notes** at `7c2b6b147d158746545699226343a3fd09b7bdeb`. No correctness-blocking finding. N001 is a non-blocking diagnostic/documentation qualification.

Reviewed the plan 208 changes against F13, F15-F18, F24, F003 and the shared-build scheduling rule. Security's marker/input/wrapper verdict and performance's launch-count/QoS verdict remain separately owned.

## B. Basis

The fresh detached `review-138-7c2b6b1` checkout is clean and pinned to the requested head. GitHub reports the same head, CLEAN, with all four CI jobs successful. Source review covered changed production modules and call sites, the new/modified tests, plan/backpressure/evidence and API/skill/harness documentation.

**Final merge-state qualification:** the last pin check still names `7c2b6b1`, but GitHub now reports **DIRTY** against current main, with the existing head's checks still green. This is a correctness verdict on the frozen head, not approval of a conflict resolution or a merge-ready claim. Reconciliation must preserve these changes and get fresh CI/exact-head review as appropriate.

No product edits, commits, pushes, checkout movement, local test/build/mutation runs or live-daemon operations. No build lock acquired. Read-only inspection confirmed this machine has executable `/usr/bin/nice` and `/usr/sbin/taskpolicy`; actual background QoS was not re-measured by this reviewer.

## C. Acceptance coverage

| Surface | Correctness disposition |
|---|---|
| F15 lineage add | Prime gate is project-scoped, dates are parsed/ordered and future ends refused; overlap checks run under the writer lock; closed/unregistered seats need no live-seat check and no role is set |
| F16 batch assign | Input/seat validation precedes writing; role/open-holding/duplicate validation and all holders documents are built under one writer lock; valid batch is one commit, receipts remain in request order |
| Single assign parity | Delegates to `assign_many` with one request and preserves its receipt/refusals, role setting, after-assign hook and window behavior |
| F003 | Fixed-clock unit test covers the millisecond collision; HTTP reassign test has no wall-clock precondition and still checks two holdings survive rebuild |
| F13 zones | Writer stamps UTC milliseconds with Z; lineage offsets normalize to UTC; CLI view uses a local offset once per view, JSON retains stored UTC |
| F17 window | Subsequent role assignment keeps the first open holding's name, reports skipped rename and avoids that launch; release-not-renaming is explicitly documented |
| F18 marker/retry | In-process marker in the worktree-specific git dir covers staging/commit and uses RAII cleanup; existing bounded foreign-index-lock retry remains, without deleting that lock |
| F24 owner | PIJ_ID, PIJ_SESSION_ID, whoami, then user fallback; discovery occurs once per invocation, not per lock poll |
| Cargo scheduling | Each command in a `--then` batch receives the wrapper array; absolute nice/taskpolicy paths and absent-wrapper behavior match the plan |
| Docs/states | New verbs/refusals/help, difficulty dispositions, owner/QoS instructions and Delivers line are present; no roadmap state flip is claimed |

The batch's all-or-nothing guarantee is the authoritative writer commit. Store indexing, hooks and renames run afterward per holding, as the existing git/store boundary requires; a post-commit indexing failure reports the committed SHA. This review does not claim a cross-system transaction spanning git and SQLite.

## D. Finding

| ID | Priority | Location | Finding |
|---|---|---|---|
| N001 | P3, non-blocking | `crates/daemon/src/http/directory.rs:773-826`; `docs/how/pij-rs-api.md` batch-assign paragraph | Mixed-invalid batches do not necessarily name every bad line in one response |

## E. N001 detail

HTTP preflight collects syntax, role-ID shape and seat-existence/liveness errors. If any occur, it returns at lines 814-819 before `assign_many` performs role existence and already-held validation. A batch containing both a missing role and a malformed line therefore initially names only the preflight errors. Service-level role errors also identify the role rather than the original line number.

The committed test already contains this distinction: `qa pij-second` names a role not created by `with_roles`, alongside the unknown-seat and malformed lines, but it asserts only the latter two diagnostics. The batch still safely writes nothing.

Treat the API's “naming every bad line” promise as phase-local, or tighten the diagnostics/test later to preserve line numbers and collect cross-phase failures. This does not block the core atomicity behavior.

## F. Evidence and limits

Exact-head CI run [42156546796](https://github.com/AI-Substrate/pij-fleet/actions/runs/42156546796) succeeds for check (22), check (24), rust-lint and rust-test. Downloaded JUnit records **1,894 tests, zero failures/errors**, including:

- `a_batch_assign_is_one_commit_all_or_nothing`
- `the_prime_records_a_closed_seats_past_holding`
- `review::review_reassign_keeps_both_holdings`
- `a_collision_moves_from_on_by_a_millisecond_without_the_wall_clock`
- `times_parse_with_a_zone_or_as_a_utc_date_and_render_locally`
- `a_held_index_lock_is_waited_for_then_refused_by_name_never_forced`
- `assign_and_release_write_holders_set_seat_roles_and_call_the_hook_once_each`

The check (22) log confirms **15 shared-build Vitest cases passed**, including the new owner and per-command nice/taskpolicy cases. The fake taskpolicy proves argument propagation; it is not itself a measurement of macOS scheduling state.

The coder reports 50/50 local reassign runs and private-daemon cost receipts at predecessor `6e40ada`. Those were read, not rerun or relabeled as final-head measurements. Exact `7c2b6b1` CI independently exercises the deterministic rewrite.

The ddocs CI stage explicitly reports UNAVAILABLE, not document validation. Coder-supplied local ddocs/static evidence is retained as such; this reviewer did not rebuild the docs. No new local check was required for the correctness verdict.

Cost surface: no new timer, poll or sweep. Marker I/O is in-process; batch writing amortizes the existing commit; skipped secondary-role rename avoids a launch; owner discovery is once per shared-build invocation; nice/taskpolicy wrap each cargo command, not each poll. The writer's existing foreign-lock retry remains bounded. Performance owns final cost/QoS receipts and the live after-deploy reading.

The marker is best-effort diagnostic information, not lock authority. This correctness approval does not waive security's assessment of its filesystem handling.

## G. Commands used

Read-only git HEAD/status/base-to-head diff and source reads, plus:

- `gh pr view 138 --json headRefOid,mergeStateStatus,statusCheckRollup,body`: requested head, CLEAN, all checks green.
- `gh run view 42156546796 --json headSha,status,conclusion,jobs`: exact SHA, completed, success.
- `gh run download 42156546796 --name junit-linux --dir <reviewer-session-files>/pr138-ci`: initial download succeeded; a redundant second download refused to overwrite the retained file, which was left unchanged.
- `gh run view 42156546796 --job 127691948358 --log`: 15 shared-build tests passed; ddocs UNAVAILABLE confirmed.

All local test/build/probe execution was avoided. CI artifacts remain under `<reviewer-session-files>/pr138-ci/`.

## H. Handover and harness feedback

No correctness changes are owed before merge. Keep N001's diagnostic wording qualification visible; retain separate security and performance decisions.

The current DIRTY merge state must be reconciled before merging; no review worktree was moved to do so.

**magicWand fulfilled:** F003 no longer spends wall-clock timing luck as test evidence, and shared-build encodes the low-priority scheduling requirement instead of relying on every seat to remember a prefix.

**difficulties:** aggregate green CI can conceal phase-local diagnostic coverage, and a passing fake QoS wrapper is not a real scheduler measurement. Both proof ceilings are explicit here rather than expanded into unnecessary local runs.
