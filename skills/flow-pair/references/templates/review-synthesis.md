# Review Synthesis — {{DELEGATION_ID}} / {{REVIEW_ID}}

> **Usage**: This is a MANUAL fill-in template. The orchestrator/reviewer populates it by hand
> (or with LLM assistance) after completing the 10-dimension review. It is NOT rendered
> programmatically by `lib/review.ts` — `lib/review.ts` only performs deterministic
> artifact-contract checks. The human/LLM judgment lives here.
>
> Replace every `{{PLACEHOLDER}}` with the real value. Delete comment blocks before filing.

---

## Header

- **Delegation**: {{DELEGATION_ID}} (run: {{RUN_ID}})
- **Review**: {{REVIEW_ID}}
- **Reviewer**: {{REVIEWER_MODEL}} (cross-model: {{IS_CROSS_MODEL}})
- **Verdict**: {{VERDICT}}
- **Reviewed at**: {{REVIEWED_AT}}

---

## Findings by severity

### Critical

<!-- Each finding: dimension · file (if applicable) · message · disposition [fix|accept|defer] -->

| # | Dimension | File | Message | Disposition |
|---|-----------|------|---------|-------------|
| C1 | | | | fix |

### High

| # | Dimension | File | Message | Disposition |
|---|-----------|------|---------|-------------|
| H1 | | | | fix |

### Medium

| # | Dimension | File | Message | Disposition |
|---|-----------|------|---------|-------------|
| M1 | | | | accept |

### Low / Info

| # | Dimension | File | Message | Disposition |
|---|-----------|------|---------|-------------|
| L1 | | | | info |

---

## Summary note

{{REVIEWER_SUMMARY}}

---

## Fix packet (if FIX_REQUIRED)

- **Fix packet**: {{FIX_PACKET_ID}} at `{{FIX_PACKET_PATH}}`
- **Allowed scope** (AC-06 — exactly the files in findings):
  {{ALLOWED_FILES_LIST}}
- **Original delegation**: {{DELEGATION_ID}}
- **References review**: {{REVIEW_ID}}

---

## Mutation-gate evidence (Dimension 0)

For each load-bearing guard verified:

| Guard | Sed expr | Tests RED | Tests GREEN (restored) | Load-bearing assertion |
|-------|----------|-----------|------------------------|------------------------|
| | | | | |
