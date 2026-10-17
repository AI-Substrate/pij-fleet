# pij-name-sweep: seat-name corpus sweep (plan 176)

**magicWand:** a corpus-review fixture that prints every word in context, beside a
sample of the words in the other slot (`gay` + `thorn`, `rubber` + `pigeon`), so
the reviewer judges the pairs people will actually see. Reading 1,954 bare words
in rows of 20 works, but the worst cases only show up as pairs.

**What shipped:**
- 422 of 1,176 adjectives, 469 of 738 nouns (84 animals plus all 385 franchise
  nouns) and all 40 ship names were removed from both runtimes and added to
  `EXCLUDED_NAME_WORDS` (983 entries). The space is 754 × 269 = 202,826 pairs, with no ships.
- The exclusion test now reports every returned word at once, checks each
  hyphenated ship word, and pins `gay` and `thorn`. It was shown red on the
  d48cde2 corpus first.
- `assets/audit.md` gives each removed word's category, and `assets/live-seats.md` lists the 40
  live seats (of 52) that use a removed word, for Jordan.

**difficulties:**
- `.omp/extensions/pij/core/memorable-id.test.ts` has literal NUL bytes in its
  adopt-seed strings. The Edit tool refuses the file as binary, `grep` prints
  nothing unless given `-a`, and `git diff --stat` shows it as `Bin`. Fix: write
  the seeds as `\0` escapes, like the Rust test does. `stretch:`.
- `node_modules/.bin/ddocs` is absent in this worktree's node_modules, which the
  backpressure recipe names as the writer, so the global `ddocs` 0.1.0 was used.
  Severity low; the plan-175 run hit the same.
- The oracle regeneration comment imported from `./.pi/extensions/...`, a path
  that no longer exists. It is fixed to `.omp/`, and the corpus header's vendored-from
  path was stale the same way.
- The removal was scripted (one data file of words by category, applied to both
  files, then a subsequence check against d48cde2). Hand-deleting about 930 lines
  in two files would have risked the runtimes diverging. `stretch:` keep the
  corpus in one data file that both runtimes generate from, so it cannot be
  vendored twice.
- Fix round 1: I read the bar as "offensive or sensitive to a person" and missed
  the in-product sense. A seat id sits next to seat states and roles in every
  status table, so `pij-ready-moose`, `pij-prime-bat` and `pij-critical-bug`
  mislead operators. Lesson: also review a corpus against the vocabulary of the
  tables it appears in. `stretch:` derive that exclusion from pij's own state,
  role and alarm enums, so a new state word can never become a name.
