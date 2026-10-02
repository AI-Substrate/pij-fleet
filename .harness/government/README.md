# Governance lives on `prime-governance`

This directory is a pointer, not a second government. The standing
`prime-governance` worktree owns governance documents; the o-prime is the sole
writer. Read the [governance branch rules](../../skills/pij/references/prime/rituals/bootstrap.md#governance-branch--the-rules)
for placement, orphan bootstrap, receipt-only requirement reconciliation and provenance.

From any pij checkout, resolve `<government-root>` without guessing a machine path:

```sh
bash harness/scripts/government-root.sh
```

The resolver prints the registered `prime-governance` worktree's
`.harness/government` directory. If the standing worktree is absent, it refuses
and prints the bootstrap command; never fall back to this directory.

Product code, numbered plan folders and everything CI/gates read remain on
main. The governance branch never merges to main; it uses no PRs or CI.
