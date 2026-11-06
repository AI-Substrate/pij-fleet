# PR #141 s09 repair review

## Current verdict

**block** at `bfc5c54fb41e9a922e30b0863c2829ee13fd4eb2`.

The reported F001-F004 are closed, and the requested F005 parser-replay coverage is present and passing. One remaining s09 issue, F006, prevents the no-holder action from serving instance-scoped requests. It is not a defect in the borrowed s05 drain implementation.

Reviewed only the one-commit `47bd3b2..bfc5c54` repair delta and the interfaces needed to verify its commands. The fresh detached `review-141-bfc5c54` worktree is clean and pinned correctly. Stacked #129/#133/#137 remain outside this correctness verdict.

## Repair dispositions

| Item | Disposition |
|---|---|
| F001 custom role definition | Closed: needed and recurring create forms carry `--packet`; replay derives required sections/fields from the actual role schema |
| F002 call_on | Closed: positional shim target and `--body-file <path>`, with project preserved |
| F003 memory-stale | Closed: file path replaces the refused shim stdin form |
| F004 split | Closed: distinct roles counted independently of interleaved instances; each holding retains role/instance; steps are ordered and rendered with `then:` |
| F005 replay proof | New real-shell splitting and real directory/send parser cases pass; native revive passes clap. This proves syntax/required-field contracts, not every command's service postcondition |
| Unshipped s15 actions | Correctly moved to `later` with a note, `commands: []`, `command: null`; replay explicitly checks those verbs are refused on this base |

`picture::command` centralizes shell-word construction. The hostile-text fixture and absence-of-files assertion were source-reviewed and pass in CI; security owns the independent S001 verdict. Performance and wording changes remain with their assigned reviewers; their adapter changes preserve the data consumed here.

## F006, P2: no-holder remediation drops the queued request's instance

Locations: `community/picture.rs`, `actions` no-live-holder branch; supporting boundary contracts at `http/directory.rs:676-700`, `delivery/role_send.rs:679-724`, `store/src/role_requests.rs:199-205`.

`actions` receives only a role ID and aggregate waiting count. When no holder is live, it always prints:

```text
pij directory assign reviewer '<seat>' --why '<why>' --project fleet
```

For a request waiting on `role:reviewer/141`, that assigns the empty instance, not `141`. Directory admission explicitly maps a bare role to instance `""`; the assign hook forwards that exact instance to the drain. The queue selector is:

```sql
AND (instance = ?3 OR instance = '')
```

With `?3 = ''`, the queued `141` row cannot be selected. The command succeeds syntactically and creates a holder, but does not deliver the queue the action says it will deliver. The new holder can then suppress the role-level no-live-holder action on the next view while the instance-specific request still waits. Similarly, a live holder of `/142` must not hide an unserved `/141` request.

This is a supported case under the live per-PR reviewer/security/performance instance workflow, not malformed input. It was found while following the instance-preservation repair and remains in s09's command generator.

**Required repair:** retain the queued instance in the action and evaluate whether that address has a live holder. The new bodiless request projection already retains the needed instance data; no body fetch or extra query is required. Add a consumer-level queue case with an instance request, a different requester/assignee and no matching holder, then verify the generated assignment drains that exact request. Also cover another instance being live. Use different requester/assignee identities so this does not conflate the separate self-send follow-up.

Proof grade: direct source/dataflow analysis, not a local reproduction. The parser replay cannot expose this semantic error: assigning a bare many-holder role is legal syntax, but it does not serve the instance request.

## Exact-head evidence

Run [42205995932](https://github.com/AI-Substrate/pij-fleet/actions/runs/42205995932) passes all four jobs at exact `bfc5c54`. Downloaded JUnit has **1,996 tests, zero failures/errors**, including:

- `every_printed_command_runs_as_written_through_the_real_parsers`
- `the_pictures_revive_line_parses_as_native_revive`
- custom needed-role, instance-preserving split and picture HTTP tests
- unchanged local-time picture rendering and C29/C30 coverage

The new test uses one `/bin/sh` invocation in an empty scratch directory to print words, then replays through real parsers and schema-derived section requirements. This review did not execute it locally or claim a full service-operation proof from that parser test.

Read-only git diff/status and dependent source reads, `gh pr view 141`, `gh run view 42205995932 --json headSha,status,conclusion,jobs` and download of its `junit-linux` artifact were used. No local builds/tests/probes, no build lock, no product changes/commits/pushes or worktree moves.

**Handover:** F001-F005 are addressed; keep blocked only for F006 and return a repaired frozen head. This asks for a correct s09 action, not reimplementation of s05.

**magicWand:** test the advertised postcondition, "assigning delivers this queue", with an instance-scoped request. Parser acceptance is necessary but not sufficient.

---

# Historical verdict at 47bd3b2

## A. Verdict and scope

**block** at `47bd3b2a9cb2fe45d0311a34b04cfc6210d3aa9f`.

Four P2 command-generation defects remain in s09. They are source-proven against the current command/schema boundaries, not claimed local reproductions.

Scope is **s09 only**: d2ee8e3, 8440d5d, 0c2df19, 8eaf5f7, 8b7cb69, 51a7dc8, db19dbf and the plan commits selected by the supplied log exclusion command. The stacked #129/s05, #137/s06 and #133/s12 implementations were not re-reviewed. Their interfaces were read only where necessary to verify the commands s09 emits.

The fresh detached `review-141-47bd3b2` worktree is clean and pinned correctly. Exact-head CI run [42184008492](https://github.com/AI-Substrate/pij-fleet/actions/runs/42184008492) passes all four jobs; JUnit records **1,994 tests, zero failures/errors**. This green result does not cover executing the invalid generated commands below.

## B. Findings

| ID | Priority | Location in `crates/daemon/src/community/picture.rs` | Finding |
|---|---|---|---|
| F001 | P2 | 438-443, 569-573 | Custom-role create recommendations omit the mandatory standing packet |
| F002 | P2 | 700-706 | `call_on` emits native send flags through the shim, which refuses them |
| F003 | P2 | 354-364 | Memory-stale action uses the stdin form refused by the selected shim |
| F004 | P2 | 603-628 | Split recommendations lose instance keys and conflate assignment rows with distinct roles |

### F001: custom creation cannot produce a valid role

The non-template `fold_needs` branch and recurring-path `unowned` branch provide title, kind, lifecycle, holders and scope/rule fields, but neither supplies `--packet` or `--brief`. The role schema requires `standing_packet` (`.dd/schemas/pij/role/schema.json:162-169`).

Filling every displayed placeholder still leaves the role invalid. For example, a missing non-template `role:engine-specialist` gets a create line with no way to fill the missing packet. This fails the new-definition path of AC3/req-27 and the recurring-owner recommendation in AC8.

The HTTP test later adds `docs-system` using a separately hand-authored command that includes `--packet p`; it does not replay the generated recommendation. Thus its passing "adding clears the prompt" assertion does not prove the recommendation works.

**Repair:** include all required definition inputs in both custom create forms and retain a regression that validates the generated definition, not merely its prefix or `--why`.

### F002: cold-memory call_on is refused before a request is created

`call_on` prints:

```text
pij send --to role:qa --body "<what you need from it>" --project fleet
```

The chosen `pij` send route is `/v1/shim/send` (`.omp/extensions/pij/core/generation-routing.ts:204-210`). The generation router forwards argv unchanged. At `crates/daemon/src/http/shim.rs:309-316`, any `--to` is explicitly refused as unsupported broadcast, even when only one destination is given.

Consequently the C30 command cannot call on a cold role with memory. Its test asserts this exact invalid string rather than admitting it through the selected consumer.

**Repair:** emit a command supported by its actual prefix, for example native `pij-rs` with native flags or the supported shim positional grammar. Verify command admission through that boundary, preserving project scoping.

### F003: memory-stale action repeats the known shim stdin failure

The new action emits `pij directory memory add ... --body-file -`. At this exact head, `http/directory.rs:781-787` refuses that form with `E-RS-ARG`; the shim does not inline stdin for directory memory. Filling the title/hook placeholders and providing stdin therefore still fails.

This is not a request to reimplement the separately owned F30/plan-209 fix. s09 must choose a currently supported form for its own runnable action, such as a file path, a short `--body` placeholder, or the native CLI form.

**Repair:** update the generator and its documented example; test the emitted action at its selected boundary rather than checking only `starts_with("pij directory memory add ...")`.

### F004: split actions do not address the holdings they propose moving

`two_domains` uses `later.role` alone for both release and reassignment and discards `later.instance`. A real holding such as `reviewer/115` therefore gets:

```text
pij directory release reviewer <existing-seat> ...
pij directory assign reviewer <new-seat> ...
```

Directory admission maps that bare role to instance `""`; release matches the instance exactly (`community/directory.rs:1058-1059`) and returns `E-RS-DIRECTORY-NOT-HELD` for the actual `reviewer/115` row. The displayed operation does not move it. The s09 HTTP fixture itself uses `reviewer/115`, but asserts only the split's role/signal/detail, not its command.

There is a related domain-count error: rows are time-sorted and then deduplicated only adjacently by role. A seat holding A/1, B/1, A/2 in that order remains A/B/A, is described as carrying three roles, and receives an extra split of its primary A domain.

**Repair:** count distinct role domains correctly and preserve the full role/instance address for every holding the split intends to move. Cover both an instance-bearing later role and interleaved instances of the same role.

## C. AC and ruling coverage

| AC | Observed coverage |
|---|---|
| AC1 picture | Holder/liveness, queued count/oldest, latest output and dated-memory summary are assembled and asserted in CI |
| AC2 runnable condition actions | Conditions are detected, but the memory-stale command fails F003 |
| AC3 needs | C29 workstream-first helper/fallback and complete-step filtering are tested; the non-template create action fails F001 |
| AC4 prime | Designated prime and separate `prime_document` indicator are present; missing document does not imply missing prime |
| AC5 local display | Local-at-instant rendering and one line per holder are tested. The pinned time 0.3.55/ADR 0001 addresses the old multithread local-offset limitation; no new local runtime probe was needed |
| AC6 cost | View path reads store/registry/documents without launching per role/seat; private evidence predates the final head and is not relabeled as a new measurement |
| AC7 docs/states | s09 complete with plan cell; req-12/24 and c-orient shipped; req-27/29 remain in-progress; full-address Delivers line present |
| AC8 C28/C30 | Required Signal field, for/against notice, boundary review and cold-memory preservation are present; call-on and split execution fail F002/F004, recurring create also fails F001 |

Core/security suggestions are held behind `--suggest` unless a need names them; bare/full-view tests cover that behavior. Security owns the view's read-only/quoting/refusal verdict. The s15 retire/merge provider remains outside this scoped review.

## D. Tests and proof limits

Exact-head JUnit confirms the picture unit tests, `the_view_shows_the_picture_and_prompts_the_prime`, workstream-first/fallback case and `the_directory_view_renders_the_picture_in_local_time` pass. Tests prove rendering and condition selection, but command assertions do not establish executable behavior: they accept missing flags, invalid prefix/argv combinations and lost instance keys.

No local builds, test suites, probe loops, schema installs or mutations were run; no build lock acquired. These failures are cross-platform command/schema contracts, so there is no macOS-only gap requiring a local check.

Commands used: read-only pinned git status/log/show and scoped source reads; `gh pr view 141`; `gh run view 42184008492 --json headSha,status,conclusion,jobs`; `gh run download 42184008492 --name junit-linux --dir <reviewer-session-files>/pr141-ci`. All four CI jobs succeeded at the exact requested SHA. The ddocs CI availability limitation is not treated as actual validator execution.

## E. Cost, handover and harness feedback

No timer/poll or per-seat process launch is added. Source shows four store queries and one registry list per view, plus in-process role/memory/step/blueprint reads. The ten-view no-launch receipt is from earlier 8440d5d sources; performance owns final read-cost and deployment measurements.

Keep blocked for F001-F004, repair s09's generators and add consumer-level regressions before returning a new frozen head. Do not expand this into fixing or re-reviewing the stacked features.

**magicWand:** execute generated next steps through the consumer selected by their prefix. A green assertion that an invalid string is unchanged is not proof of a runnable command.
