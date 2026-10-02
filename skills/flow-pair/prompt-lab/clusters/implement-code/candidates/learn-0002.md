# Learning Candidate — learn-0002

- **Cluster**: implement-code
- **Run**: 2026-07-02T22-32-26Z-github.com-AI-Substr
- **Delegation**: dlg-0002
- **Miss type**: implement-code
- **Created at**: 2026-07-03T00:55:20.835Z

## Summary

Advertised CLI flags need effect-asserting regression tests, not just parse tests — silent parse-but-drop shipped a green suite

## Evidence

- rev-0002 HIGH: --permissions/--cwd parsed by cli-args but never forwarded to AgentRunConfig
- 50 parse tests green while both flags were no-ops
- caught only by reviewer's hermetic run.json probe

## Candidate prompt delta

Packet quality bar: for every user-facing flag the task advertises, require one test asserting the flag's EFFECT on the run artifact/output (e.g. run.json fields), written red-first

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
