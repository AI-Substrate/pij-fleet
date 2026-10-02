# Worker Fix Packet — {{FIX_PACKET_ID}}

> **Usage**: This is a PROGRAMMATIC template rendered by `lib/review.ts`
> `generateFixPacket()`. Every `{{PLACEHOLDER}}` is substituted at generation time.
> Do NOT edit this template's placeholders — they must match the `subs` record in
> `generateFixPacket()` step 6 exactly.

---

## Context

- **Original delegation**: {{DELEGATION_ID}}
- **Review**: {{REVIEW_ID}} (verdict: FIX_REQUIRED)
- **Run**: {{RUN_ID}}
- **Fix packet**: {{FIX_PACKET_ID}}

---

## Mission

Fix **ONLY** the issues listed in the findings below. Do not refactor, rename, or
reorganise code outside the findings scope. Do not add new features. Do not touch files
outside the allowed scope list.

---

## Allowed scope (AC-06)

Verify your working directory BEFORE the first edit — run `pwd` and confirm it
is the repo root this packet targets; any other checkout/worktree → STOP,
report BLOCKED (never "fix" the path yourself).

You may only write to these files (exactly — no others):

{{ALLOWED_FILES_LIST}}

Any change to a file NOT on this list is an automatic out-of-scope finding in the
next review cycle.

---

## Fix dossier (findings you must address)

{{FINDINGS_SUMMARY}}

Address every finding above. For each: state what you changed and why it satisfies
the finding in `execution.log.md`.

---

## Forbidden paths

Do **NOT** touch any of:

- `.the-flow-state.json`
- `the-flow.json`
- `the-flow.md`
- `.flow-pair/` (any file under this dir)

---

## Report back

When done:

1. Run the consuming repo's own gates — typecheck, lint, and the targeted tests for
   every file you changed, using THIS repo's own commands (check its README/justfile/
   package scripts; ask the orchestrator if unnamed). All gates must pass before
   reporting. Never assume another repo's recipes exist here.
2. Confirm mutation checks on any load-bearing guard you touched.
3. Reply with a Worker Report per the orchestrator-worker protocol
   (`{{SKILL_ROOT}}/references/orchestrator-worker-protocol.md` — absolute path,
   resolves outside the repo root). Include the literal test-runner pass line
   (e.g. `Tests N passed (N)`) and the exact mutations used for mutation gates.
