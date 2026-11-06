# PR 130 routing-table review

Tags: reviewer, routes, compact, idempotency, exact-head, SQLite

Hook: a callback outside request-id deduplication can duplicate side effects even when the primary operation executes once.

Verdict: block at `dcc3e526043b2aa3206eae0ea036876183fb9d99`. Report: `../outputs/2026-11/pr-130-verdict.md`; reusable regression beside it as `pr-130-replay-regression.rs`.

F001 P2 reproduces through native `/v1/send`: replay one compact request with the same msg_id. The durable queue retains one compact row, but held FYIs grow from two (one assign plus one compact table) to three. `http/mod.rs:2056-2060` unconditionally runs the callback on successful replay receipts; `http/routes.rs:227-235` creates a fresh FYI ID. Repair must durably associate the side effect with the control identity, including replay after FYI claim. A queued receipt is not a new-admission signal.

Three committed routing HTTP controls pass against immutable pinned sources. Exact-head CI run 42139627097 passes all four jobs. Project docs validate/render, local-path and lockfile sensors pass, and the plan validates without contradictions. No dependency installs or symlinks, product edits, commits, pushes, prod-daemon changes or pane input.

The compact callback's holding assertion is not post-compact context-consumption proof. Copilot might claim the FYI on `/compact` before compaction; this remains a non-blocking proof ceiling under PM's held-FYI acceptance, not another demonstrated bug.

The supplied warm target was absent; focused tests used the extant fleet-190 target through shared-build. A scratch trigger-guard mutation was deferred because of the fleet backoff; its waiting wrapper terminated with exit 143 before acquiring the lock or running cargo. No mutation proof claimed; source restored exactly. No lock is held and no further local builds, tests or probes run under the backoff. The aborted local full-harness build's private target was cleaned up; the shared target was untouched. Quick harness checks remain dependency/tooling-limited; Copilot native passes.

**magicWand:** preserve the native compact replay regression, with independent control-job and FYI counts, as a permanent repair test.

**difficulties:** unavailable supplied target, shared-target contention, intentionally missing node_modules, and no observed native post-compaction claim boundary. No further build is needed for the reproduced blocker.
