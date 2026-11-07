# Fold a ruling into the docs

A ruling (from the operator, or the prime within the operator's rulings) changes what a PRD, blueprint or workstream says. Fold it in on the branch of the plan it affects, or on its own small PR if it affects none. Read [README.md](README.md) first.

## Steps

1. **Find what it touches.** Search the three docs for the requirement, component or step the ruling names. The filtered `ddocs links` in [README.md](README.md#links) shows which components and steps point at a row, so they get checked too.
2. **Amend, never delete.** For each affected row, change only the fields the ruling changes:
   ```bash
   ddocs set 'docs/project/prds/001-x.dd.json#requirements/req-03/statement' "$(cat /tmp/req-03.txt)"
   ddocs set 'docs/project/prds/001-x.dd.json#requirements/req-03/done_when' "$(cat /tmp/req-03-done.txt)"
   ```
   Keep the text in a file when it has backticks or `$`; inside double quotes the shell runs them.
3. **New requirement:** add a row with the next free id, `state: "planned"` and `source` naming the ruling. If today's code already does part of it, use `in-progress` and say in `note` what exists and what doesn't.
4. **Ruled out:** set the row's state to `dropped` (a step: `abandoned`) and put the ruling in its `note`. The row stays.
5. **Changelog.** Add one row per doc you changed, naming the ruling: `{"date": "...", "by": "<your seat>", "change": "req-03 reworded per ruling C8: ..."}`.
6. **Validate all three docs**, not just the one you edited, since links cross files. Then `ddocs build` each one you changed.
7. **Tell the owner.** If a plan's acceptance criteria quote the old wording, send pm/001 the row ids that changed so the plan is updated in the same PR.

Example: workshop 002's answers became `req-22` to `req-25` in `docs/project/prds/001-pij-community.dd.json`, plus one changelog row naming the workshop.
