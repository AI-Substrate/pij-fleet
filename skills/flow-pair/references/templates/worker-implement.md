# Worker Packet — {{DELEGATION_ID}}

**Run**: {{RUN_ID}}
**Delegation**: {{DELEGATION_ID}}
**Phase**: {{PHASE}}

---

## Mission

{{TASK_DESCRIPTION}}

---

## Completion Discipline — finish the WHOLE phase in this one delegation

**Implement every task in this phase before you report.** This delegation covers the
entire phase, not a slice of it. Do **not** complete a couple of tasks and hand back —
that fragments the work, wastes a full orchestrator round-trip per slice, and loses your
warm context. Work straight through the task list start-to-finish in this single run.

- Only report `"outcome": "COMPLETE"` when **every** task in the phase is done and the
  self-check gates pass.
- Use `"PARTIAL"` / `"BLOCKED"` **only** for a genuine hard blocker you cannot resolve
  (missing decision, contradictory spec, failing dependency) — never as a routine
  "checkpoint" pause. If you can keep going, keep going.
- Don't stop to ask for confirmation between tasks; the packet IS your authorization for
  the full phase. Re-read the allowed scope if unsure, then proceed.

---

## Repo Root

```
{{REPO_ROOT}}
```

Work inside this repo root. All relative paths below resolve from here.

## Flow-Pair Skill Root

```
{{SKILL_ROOT}}
```

The flow-pair skill's own install root (absolute — NOT inside the repo root above).
Protocol references cited in this packet resolve from here:

- `{{SKILL_ROOT}}/references/orchestrator-worker-protocol.md` — packet/report schema + allowed/forbidden-paths contract
- `{{SKILL_ROOT}}/references/review-rubrics.md` — review rubric your work will be judged against

---

## Forbidden Paths (NEVER edit these)

The following paths are owned by the orchestrator. **Do not read, write, or delete them.**

{{FORBIDDEN_PATHS}}

If you are about to touch any of these, stop and re-read the allowed scope.

---

## Allowed Scope

You may ONLY create or modify files within:

{{ALLOWED_PATHS}}

Stay inside this scope. Do not touch anything outside it without explicit permission.

---

## Context

### Phase Plan Section

{{PLAN_PHASE_CONTENT}}

---

### Tasks

{{TASKS_CONTENT}}

---

### Execution Log

{{EXEC_LOG_CONTENT}}

---

### Cluster Learnings

{{LEARNINGS_CONTENT}}

---

## Report Schema

When your work is complete, send a report via `pij_send` using this exact JSON schema:

```json
{
  "delegationId": "{{DELEGATION_ID}}",
  "outcome": "COMPLETE | PARTIAL | BLOCKED",
  "summary": "1–3 sentences describing what was done and the current state",
  "filesChanged": ["path/to/file1.ts", "path/to/file2.ts"],
  "testsRun": 0,
  "testsPassed": 0,
  "gatesClean": true,
  "notes": "optional — blockers, decisions, questions for the orchestrator"
}
```

---

## Stop Conditions

- **Finish the entire phase before reporting** (§ Completion Discipline) — do not hand back after a couple of tasks; `COMPLETE` means the whole phase is done
- **Run the consuming repo's own gates before reporting** — typecheck, lint, and the
  targeted tests for every file you changed, using THIS repo's own commands (check its
  README/justfile/package scripts; ask the orchestrator if unnamed). Never assume
  another repo's recipes exist here.
- **Verify your working directory BEFORE the first edit** — run `pwd` and confirm it is
  the repo root this packet names (allowed paths resolve relative to it). If it is any
  other checkout/worktree, STOP and report BLOCKED — never "fix" the path yourself.
  (Encoded from a live incident: a worker edited the wrong checkout.)
- **Do NOT edit forbidden paths** under any circumstances
- **Do NOT write `.flow-pair/`** and do NOT read any ledger files under `.flow-pair/` —
  the orchestrator owns the entire ledger directory. Exception: you may read **only this
  packet file** (the one you are currently executing from, under `.flow-pair/.../prompts/`).
  Never read `events.jsonl`, `run.json`, delegation records, or other ledger files.
- **Send your report via `pij_send`** with the schema above before stopping
- If you hit a blocker you cannot resolve, set `"outcome": "BLOCKED"` and describe it in `"notes"`
