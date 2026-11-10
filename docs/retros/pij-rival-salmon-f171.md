# pij-rival-salmon: F171 (2026-11-10)

**magicWand:** one table of kinds and their folders that every reader
shares: the claim floor, `pij delivered`'s release by merged paths, and the
help. Today the known kinds' folders live in `claims::folders`, other kinds'
in `seed_folders`, and `numbers_in_paths` still walks only the known ones. So
a `usage` claim is now numbered past its folder but is not released at merge.

**What happened:** `pij claim usage` started at 001 while
`docs/project/usage/001-019` already existed, because only known kinds had a
folder to scan. Other kinds are now seeded from `docs/project/<kind>`, then
`docs/project/<kind>s`. Both shapes are real (unimatrix has `usage` and
`benchmarks`). When neither exists, `docs/<kind>` and `docs/<kind>s` are read
only to warn.

**Found on the way:**
- "Where a folder can't be resolved, warn" had more than one fair reading. A
  two-line question to the PM with a concrete proposal settled it in one turn,
  before any of the warning code was written.
- The warning is advisory in both directions. Removing it changes no
  number and no outcome, so it reads the folders leniently: a folder it can't
  read warns nothing, unlike a seed folder, which refuses when unreadable.
