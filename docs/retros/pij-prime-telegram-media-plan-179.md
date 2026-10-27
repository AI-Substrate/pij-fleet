# pij-prime-telegram-media: Telegram media from pij-rs (plan 179)

**magicWand:** a "lost work" sensor. #49 was green in CI, live-canaried by
deploying a detached commit, and then silently undone by the next bounce from
main, because it was never merged. Nobody noticed for two weeks, and Vaughan's
memory ("I thought it did") was the only trace. `just doctor` (or the bounce
itself) should warn when the running daemon was built from a commit that is
not an ancestor of `origin/main`, and `harness checks` could list open PRs older
than N days that were canaried. Either would have kept this from being lost.

**What shipped:**
- #49 restored by cherry-pick (author kept), one doc conflict.
- sendPhoto / sendVideo / sendAnimation / sendDocument by type, chosen at
  enqueue and recorded on the job; one sendDocument fallback on a 400 in the
  same attempt; `telegram.outbound-media-delivered` names the method used.
- `--file` repeats (≤10) and combines with a body: body first, one job per
  file, in order; all-or-nothing staging.
- Inbound media is a notice plus `telegram.inbound-media-unsupported` instead
  of a silent drop; saving it is #70.
- RED shown, 9/9 mutants killed, including token redaction (M7 leaked the
  token into the error the loop logs).

**difficulties:**
- The deep look found the answer in an open PR, not in the code. Searching
  the code and git history of main found nothing; `gh pr list --state all`
  did. Severity medium: a seat told to "check if it exists" will search code
  first. `stretch:` the pij skill's deep-look checklist names open PRs.
- `git push origin` → 403: this account has read-only on AI-Substrate/pij-fleet;
  #49 came from the fork vaughanknight/pij-fleet. Found by reading #49's
  `isCrossRepository`. Severity low, now a stored memory.
- A fresh worktree has no `node_modules`, so `harness checks` failed six stages
  with exit 127 ("tsx: command not found") before any real check ran. Then
  `npm ci` failed once with ETIMEDOUT (-60) under load and succeeded on retry
  with longer fetch timeouts. Severity medium. `stretch:` `harness checks`
  says "run npm ci" when `node_modules/.bin` is missing (observed).
- `harness checks` truncates a failing stage's output to its tail: the rust
  stage said only "test failed, to rerun pass -p pij-daemon --lib", the
  failing test's name was cut off, and the rerun passed. Severity medium
  (observed). `rust-check` also stops at the first failing crate, so the
  crates after pij-harnesses were never run until `--no-fail-fast`.
- Machine load1 ran 20–68 for most of the session (other seats building).
  Timing tests in pij-daemon, pij-tmux, pij-transport and pij-harnesses
  flaked under it; each passed serially or alone, and the harnesses ones fail
  on clean main too. Proving "not mine" took four extra cargo runs and a
  throwaway worktree. `stretch:` a `--serial-retry` mode for rust-check that
  reruns a failing crate with `--test-threads=1` and reports flake vs fail.
- `harness observe --kind friction` is refused; the kinds are difficulty,
  magic-wand, gift, insight, coordination, improvement-suggestion,
  confusion, win. Papercut.
