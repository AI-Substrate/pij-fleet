# pij-codex-footer: Codex 0.164 footer with a session title (plan 177)

**magicWand:** a recogniser corpus: one fixture per harness version, run through
every recogniser (composer, readiness, model, cwd) by one table test, with
`unrecognized` failing loudly in that test. Each new harness release would cost
one recorded capture, not one incident. Plan 177 is the second Codex footer
incident in seven weeks (2026-09-08 pane %4409, 2026-10-24 pane %100), and both
were found by a message that never arrived.

**What shipped:**
- `parse_codex_footer` in `crates/harnesses/src/composer.rs` is the one Codex
  footer reading, used by composer recognition, readiness, `observed_model` and
  `observed_cwd`. The cwd is the first path segment after the model; the session
  title is ignored, even when it contains `·` or looks like a path.
- A usage-only footer (`85% context left`) now reports no cwd. It used to report
  the usage text as the cwd.
- Fixture `2026-10-24-codex-v0.164-pane-100.txt` holds the bottom lines only.
  Five tests were red on 7babd01, and a mutation back to `rsplit_once` turned four
  named tests red.

**difficulties:**
- The plan named the "context/tokens variants" as existing footers, but no
  fixture or capture of one exists in the repo; the test footer is synthetic.
  Severity low. `stretch:` record one when seen live.
- The guide's `CodexFooter{model, effort, cwd}` contract carried an `effort`
  field nothing reads, and an unread field fails `clippy -D warnings`. It was
  dropped. Contracts that name fields could note which consumer reads each one.
- `node_modules/.bin/ddocs` is still absent in a fresh worktree (third plan in a
  row); the global `ddocs` was used.
- No docs/how page described Codex readiness or composer recognition, so AC4's
  "wherever it is documented" had no target. A section was added to
  `docs/how/pij-pane-signals.md`.
