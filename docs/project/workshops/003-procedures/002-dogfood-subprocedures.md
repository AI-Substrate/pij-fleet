# Workshop 003, input 002: building sub-procedures from targeted questions

**Date**: 2026-11-23. **From**: a procedure author's dogfood report in another pij project (accelerated-decomposition).
**Feeds**: PRD 005 req-01 (`proven_in_practice`) and req-23 (the drafting pattern in PROC-meta-0001).

## What happened

The author drafted a top-level procedure from the project's main workflow document alone, marked each step's status and
listed its gaps. It then sent each of five specialists one message of lettered questions, (a) to (e), each tied to a named
step and gap id. All five answered within about 15 minutes, each in a file that answered exactly those letters, with
citations. From those files the author wrote five sub-procedures without reading any specialist's source area. Two gaps
closed, and one end state was renamed.

## What it suggests

- **A drafting pattern:** draft from the main source, mark the gaps, send lettered questions keyed to step and gap ids,
  and expect answer files rather than messages.
- **`proven_in_practice` beside `status`:** status says a step exists; `proven_in_practice` says where and when it last
  ran for real. Several steps there were "implemented" but had never run on real input.

## Then

The operator ruled on 2026-11-24 to build PRD 005 in one PR and take the open questions' proposed answers as decided; see
the PRD's `open_questions` and changelog.
