# Learning Candidate — learn-0001

- **Cluster**: implement-code
- **Run**: 2026-08-28T06-03-39Z-github.com-AI-Substr
- **Delegation**: dlg-0001
- **Miss type**: implement-code
- **Created at**: 2026-08-28T07:30:01.756Z

## Summary

Fail dispatch when a supplied tasks directory yields no tasks

## Evidence

- The rendered packet said no tasks found and omitted the ruled consumer payload contract
- coder implemented the narrower plan section
- independent review found three high-impact contract gaps

## Candidate prompt delta

When --tasks-dir is supplied and compiles to zero tasks, dispatch must fail loudly or require explicit allow-empty; never render a successful packet with no tasks found

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
