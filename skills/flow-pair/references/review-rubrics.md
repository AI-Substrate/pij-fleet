# Review Rubrics

Applied to every worker delegation at the `REVIEW` stage. Verdict model: `APPROVE` / `APPROVE_WITH_NOTES` / `FIX_REQUIRED`.

> ## Dimension 0 — Test quality (mutation-resistance) · **MANDATORY for any CODE delegation**
>
> **Why this is first.** In flow-pair the *worker* — the cheaper, less-capable model —
> writes both the implementation **and** its tests. The tests therefore share the
> worker's blind spots; they are **not** an independent quality signal. A green suite
> authored by the source of the bug proves almost nothing. **Tests are a critical
> failure mode for agents that trust them.** Treat worker-authored green tests as
> *suspect until proven non-vacuous*. (Proven live in Phase 2: all 4 gates were green
> while a CRITICAL data-loss bug sat in the code; the cross-model reviewer caught it.)
>
> **The required check — "would a test fail if the fix were reverted?"**
> For each behavioural claim (a fix, a guard, an invariant), the reviewer/orchestrator
> MUST establish that *some* test goes RED when the behaviour is removed. Two methods:
> 1. **Empirical (preferred)** — `just flow-pair-mutate <file> '<sed-ERE-expr>'`: backs up
>    the file, applies the mutation, asserts the suite goes RED, restores byte-identical,
>    and asserts GREEN again. Stays green under mutation ⇒ the tests do **not** guard the
>    behaviour ⇒ `FIX_REQUIRED`.
> 2. **Reasoned** — only when a clean mutation is awkward: name the *exact* load-bearing
>    assertion that flips when the behaviour is reverted, and why. It must be a
>    **negative / state** assertion (e.g. `writeWasCalled === false`, `events.length === 0`),
>    never a truthiness check.
>
> **Weak-test red flags** (any one ⇒ the test is suspect, dig deeper):
> - asserts only `result.ok === true` / truthiness; never exercises the failure branch;
> - no negative or state assertions (only "it returned something");
> - failure path is pure fake-fs with no real-fs counterpart;
> - lenient `OR` regexes on error messages (`/a|b/`) doing load-bearing work;
> - test count rose but every new test is happy-path;
> - the value the test asserts was not independently re-derived from the code under test.
>
> **Verdict rule:** a CODE delegation **cannot be approved on green gates alone.**
> Test quality is itself a gate. Unproven test quality on a load-bearing fix is a
> mandatory `FIX_REQUIRED`.

---

## Dimension 1 — Scope

**What the reviewer checks.** Compare the set of files the worker actually modified (from
`git diff HEAD --name-only` or the captured diff) against the delegation's `allowedFiles`
list. Any file modified outside that set is an out-of-scope change. This includes accidental
edits to `package.json`, `tsconfig.json`, domain docs not mentioned in the packet, or other
modules.

**Concrete example.** A dispatch packet scoped to `skills/flow-pair/lib/review.ts` and its
test. The worker also edits `skills/flow-pair/lib/ledger.ts` to add a convenience method.
Even if the edit is benign, it is out-of-scope — it adds a new surface without a task, plan,
or test for the addition.

**Severity mapping.**
- `critical` — worker modifies files outside `allowedFiles` AND those changes are semantically
  load-bearing (e.g. changing a public API others depend on). Automatic `FIX_REQUIRED`.
- `high` — out-of-scope edits that are additive but untested, or touch shared constants.
  `FIX_REQUIRED` unless trivially revertable.
- `medium` — cosmetic or formatting changes outside scope. `APPROVE_WITH_NOTES`.
- `low` / `info` — observation only, no action needed.

---

## Dimension 2 — Contract

**What the reviewer checks.** Do the exported types, function signatures, and class shapes
match what the tasks.md specified? Compare: parameter names and types, return types,
`interface` shapes, exported constant names, and the schema structures in `schemas/*.json`.
A mismatch breaks callers that were coded to the spec (the orchestrator, the CLI, future
phases).

**Concrete example.** The tasks spec says `generateFixPacket(opts: FixPacketOpts)` returns
`{ ok, packet?, error? }` where `packet.allowedFiles: string[]`. The worker returns
`packet.allowed: string[]` instead. The orchestrator that inspects `packet.allowedFiles` gets
`undefined` — a silent data-loss bug.

**Severity mapping.**
- `critical` — a public-facing return type or required parameter is wrong or missing. `FIX_REQUIRED`.
- `high` — an optional field is missing or has the wrong shape. `FIX_REQUIRED`.
- `medium` — internal naming differs from spec but external contract is intact. `APPROVE_WITH_NOTES`.
- `low` — minor naming style divergence not affecting callers. `APPROVE`.

---

## Dimension 3 — Plan-alignment

**What the reviewer checks.** Does the implementation match the phase plan spec
(`flow-pair-plan.md` phase section) and the accepted `tasks.md`? Look for: step-order
deviations (e.g. P9 violated), missing sub-tasks, task descriptions reinterpreted, or whole
deliverables swapped out for alternatives not discussed with the orchestrator.

**Concrete example.** tasks.md specifies "write `fix-NNNN.json` (metadata) AND `fix-NNNN.md`
(content)" but the worker writes only `.md`. The `.json` is missing; downstream readers that
parse JSON get a 404. This is a plan-alignment failure.

**Severity mapping.**
- `critical` — a deliverable entirely missing or functionally wrong per the plan. `FIX_REQUIRED`.
- `high` — step order violated (e.g. P9 persists after instead of before). `FIX_REQUIRED`.
- `medium` — minor deviation that doesn't break the contract (e.g. slightly different constant name). `APPROVE_WITH_NOTES`.
- `low` — cosmetic difference from spec prose. `APPROVE`.

---

## Dimension 4 — Acceptance Criteria (ACs)

**What the reviewer checks.** For each explicitly numbered AC in tasks.md (AC-05, AC-06,
etc.) — is it exercised by a test? Run: check that each AC appears in the test file with a
load-bearing assertion, not just a comment. Unexercised ACs are an automatic `FIX_REQUIRED`
regardless of test count.

**Concrete example.** AC-06 says `allowedFiles = exactly the files in findings`. The worker
adds a test `expect(packet.allowedFiles.length).toBeGreaterThan(0)`. This is truthy-only —
it doesn't prove scope is *bounded*. A test that asserts `not.toContain("lib/other.ts")` is
required.

**Severity mapping.**
- `critical` — an AC has no test at all. `FIX_REQUIRED`.
- `high` — AC has a test but only a truthy assertion; the guard is vacuous. `FIX_REQUIRED`.
- `medium` — AC is exercised but the test is weak (no negative assertion). `APPROVE_WITH_NOTES`.
- `low` — AC tested adequately; minor style concern. `APPROVE`.

---

## Dimension 5 — Tests (see Dimension 0 for code; this dimension covers doc/config)

**What the reviewer checks.** For CODE delegations, Dimension 0 is the gate — see above. For
DOCUMENT or CONFIG delegations (e.g. filling a rubric, updating a schema), check that the
change is *syntactically valid* (JSON parses, Markdown renders), self-consistent (all referenced
fields are defined), and that there is at least one mechanical verification (e.g. `just typecheck`
or `npx ajv validate`). A document-only phase with no mechanical check is still suspect.

**Concrete example.** Worker adds a new `oneOf` branch to `event.schema.json` but forgets
`additionalProperties: false`. Downstream validators silently accept events with extra fields,
hiding bugs. A `just typecheck` that compiles the TypeScript union catches the inconsistency.

**Severity mapping.**
- For code: delegate entirely to Dimension 0 (Dimension 5 is a no-op).
- For docs/config: `critical` = syntactically broken; `high` = missing required field;
  `medium` = inconsistent cross-references; `low` = style.

---

## Dimension 6 — Domain-currency

**What the reviewer checks.** After any API change, schema addition, or new public type — was
`skills/flow-pair/references/` updated to reflect it? Specifically: `ledger-schema.md` when
the ledger adds an event type or record shape; `architecture.md` when a new module is added;
`domain.md` when the domain boundary changes. Stale reference docs poison future agents that
read them as ground truth.

**Concrete example.** Phase 6 adds `review.recorded` and `fix_packet.written` events to
`LedgerEvent`. The worker does not update `references/ledger-schema.md`. The next agent who
reads that file doesn't know these events exist and may re-invent them or miss them in queries.

**Severity mapping.**
- `critical` — a new exported type or event is not documented anywhere in references/. `FIX_REQUIRED`.
- `high` — docs exist but have wrong shapes or missing required fields. `FIX_REQUIRED`.
- `medium` — docs exist and are roughly correct but lack detail. `APPROVE_WITH_NOTES`.
- `low` — minor prose improvement possible. `APPROVE`.

---

## Dimension 7 — Progress log

**What the reviewer checks.** Does `docs/plans/<phase>/execution.log.md` exist? Does it
describe what was actually done, not just what was planned? The log must contain: a list of
files changed, what each task accomplished, any decisions made, and the gate results (test
count, typecheck, lint). A missing or empty log is an automatic `FIX_REQUIRED` — it is the
primary artifact-contract check (AC-05) mechanized in `lib/review.ts`.

**Concrete example.** A worker completes all 7 tasks but the execution log says only
"Implemented as per tasks.md." with no file list, no decisions, and no gate output. A reviewer
reading this cannot confirm what was done without re-running the full suite, and cannot learn
from it.

**Severity mapping.**
- `critical` — log is absent entirely. Automatic `FIX_REQUIRED` (AC-05).
- `high` — log exists but has no gate results or decision rationale. `FIX_REQUIRED`.
- `medium` — log exists, gates present, but terse (no decision narrative). `APPROVE_WITH_NOTES`.
- `low` — log is complete; minor style improvement possible. `APPROVE`.

---

## Dimension 8 — Regression

**What the reviewer checks.** Run `just flow-pair-test` (or the relevant test recipe) before
and after the delegation. Does the pre-existing test count stay the same or higher? If any
previously-passing test now fails, that is a regression. Check the full suite, not just the
newly added tests. Also check `just typecheck` and `just lint` — a type error introduced
elsewhere is a regression even if no test caught it.

**Concrete example.** Phase 6 worker adds `file?` to `ReviewFinding`. One of the Phase 2
tests that builds a `ReviewFinding` literal now gets a TypeScript warning. The test still
passes (TypeScript is lenient at runtime) but `just typecheck` exits non-zero. This is a
regression.

**Severity mapping.**
- `critical` — previously-passing tests now fail. `FIX_REQUIRED`.
- `high` — typecheck or lint newly fails. `FIX_REQUIRED`.
- `medium` — no failures but a pre-existing warning level increased. `APPROVE_WITH_NOTES`.
- `low` — no regressions; cosmetic diff in tool output. `APPROVE`.

---

## Dimension 9 — Prompt-follow

**What the reviewer checks.** Compare the packet's explicit instructions (MUST / MUST NOT /
KEY DIRECTIVE) against what the worker delivered. Any explicit instruction the worker ignored
or contradicted is a finding. Contrast with Dimension 3 (plan-alignment): Dimension 3 is
about the tasks spec; Dimension 9 is about the prompt/packet's runtime directives.

**Concrete example.** The packet says "MED-1: BOTH writes MUST happen AFTER the
`fix_packet.written` event append + `{ok}` check, inside the P4 try/catch." The worker
writes `fix-NNNN.json` before the event append. This is an explicit instruction violated —
Dimension 9 finding, also a P9 violation (Dimension 3).

**Severity mapping.**
- `critical` — a MUST directive explicitly violated (e.g. security guard removed, P9 broken,
  forbidden file touched). `FIX_REQUIRED`.
- `high` — a SHOULD directive not followed with no stated reason. `FIX_REQUIRED`.
- `medium` — a directive followed in spirit but not literally. `APPROVE_WITH_NOTES`.
- `low` — worker followed a reasonable interpretation; directive was ambiguous. `APPROVE`.

---

## Dimension 10 — Learning

**What the reviewer checks.** Did the worker capture at least one key decision, non-obvious
implementation choice, or design trade-off in `execution.log.md`? In flow-pair, the
execution log is the primary learning artifact for the orchestrator and for future agents.
A delegation with no captured decisions wastes the expensive cross-model reasoning cycle.

**Concrete example.** The worker chose `readdirSync(...).filter(f => f.endsWith(".json")).length`
for ID allocation (OQ-01 single-writer pattern). This decision has a known concurrency
limitation. If it is not documented, future agents may encounter the limitation without
context and either re-debate it or make the same mistake.

**Severity mapping.**
- `critical` — no decisions captured despite multiple non-trivial choices. `FIX_REQUIRED` if
  the choices affected correctness or contract.
- `high` — key decision captured but incorrectly (wrong rationale or wrong outcome stated). `FIX_REQUIRED`.
- `medium` — decisions partially captured; important trade-offs omitted. `APPROVE_WITH_NOTES`.
- `low` — all key decisions captured; minor wording improvement possible. `APPROVE`.
