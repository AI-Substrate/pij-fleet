# pij-tg-retry — Telegram retry backoff, inbound never starves (plan 183)

**magicWand:** a queue fake that keeps the real queue's ORDER. `FakeQueue::retry`
pushes the retried row to the back of `live`, and its claim takes the first
eligible row, so every sidecar test against the fake sees a retried head
overtaken by its siblings no matter what the store does. The ordering half of
this plan could only be proven against SQLite, which meant a test in a third
crate (pij-sidecars may not dev-depend on sqlx). A fake that agrees with
`ORDER BY id` would have let it live beside the worker it tests.

**What shipped:**
- Backoff 2 s doubling to 300 s, `retry_after` honoured when longer, 10 attempts
  then `telegram.outbound-gave-up`; a `retry_after` over 1 h fails at once.
- `SqliteQueue::retry` rounds its deadline UP (it truncated: 100 ms became 0 s).
- Telegram rows on one chat claim strictly in id order (claim SQL scoped to
  `sidecar:telegram:%`, on index `jobs_serial`).
- Two loops, outbound and inbound; connect/API/upload timeouts 10/30/600 s.
- `Queue::enqueue_all` → `BatchEnqueue::{Queued, Conflict}`, one transaction:
  N1 (reused msg_id refused by name) and N3 (atomic body+files) in one seam.
- N2 7-day sweep of failed copies; N4 CLI keeps a symlink's typed name.

**difficulties:**
- *Ordering needed the queue (high).* The body→files order relied on the 0 s
  truncation the issue named; a real backoff breaks it in the claim, which is
  outside the worker. Raised at the first seam (Q1) before code: approved,
  scoped to telegram kinds, with an EXPLAIN receipt as a condition.
- *The approved port shape could not be atomic (medium).* `Vec<Enqueued{…,
  live_payload}>` only reports a conflict after the batch's fresh rows commit.
  Moved the decision inside the transaction (`Conflict` rolls back) and told the
  prime, who accepted it. Lesson: an "all or nothing" requirement belongs in
  the port's return type, not in what the caller does with it afterwards.
- *arch_drift caught a forbidden dev-dep (low).* Adding `sqlx` to pij-sidecars
  for one end-to-end test tripped `the_live_workspace_matches_the_ratified_graph`.
  Good guard; the test moved to crates/daemon/tests with the prime's OK (Q3).
- *Red on main for new surfaces (low).* Tests that name new API cannot compile on
  fd7d3bb, so the red run used main-API variants (run_once for run_outbound,
  no timeouts field) and quotes compile errors for the rest. `stretch:` a
  `just red-on <sha> <test-files>` recipe that builds the scratch worktree,
  copies only test files and keeps a separate target dir.
- *Seam noted, not changed:* the daemon builds the attachments path from a
  literal (`crates/daemon/src/lib.rs` "telegram-attachments") while the sweep
  uses `pij_sidecars::telegram::ATTACHMENTS_DIR`; the CLI e2e test pins the
  same path. Out of fence; one-token follow-up.
- *Load-sensitive tests read as failures (medium).* Each local `just rust-check`
  run had one failure outside this diff (pij-harnesses `prompt_hook_*`;
  pij-daemon `a_live_idle_claude_frame_under_a_long_stale_working_is_refused`).
  Each passes 5/5 alone, and CI was green. Captured as `harness observe` DL-001.
- *`harness checks` without node_modules says FAIL, not NOT RUN (low).* A
  worktree where the packet forbids `npm install` gets five red TS stages that
  are really absent tooling (CONF-001). `stretch:` have the checks extension
  mark a stage NOT RUN when `node_modules/.bin` is missing.

**Review round (pij-review-pr83):**
- *B1 was mine twice over.* I named the Err-path edge in my own report, called it
  "rare", and shipped a Conflict filter that checked ONE live row when a request
  spans many. A conflict is per row; the shared resource (a staged path) is per
  request. The fix put the check in the release function's signature
  (`release_staged(docs, &LiveCopies)`), so no caller can delete without
  saying what is live. Lesson: when a delete needs a guard, make the guard a
  required argument, not a filter at one call site.
- *My safety argument was a number I never re-derived.* "13.5 minutes" was the
  backoff sum, not a live job's lifetime (retry_after and upload time). The
  brake made the number unnecessary; the doc now states the real worst case.
- *Load-sensitive daemon test, third sighting.*
  `a_live_idle_claude_frame_under_a_long_stale_working_is_refused` failed again
  under a parallel multi-crate run and passed 640/640 twice alone. Likely cause:
  its fake `tmux` is a shell script read under the capture timeout, so a slow
  capture reads as "no idle evidence" and the send is allowed. Out of fence.
