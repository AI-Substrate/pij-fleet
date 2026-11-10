# delivery memory

- [2026-11-04 req-07/08/22 collisions](2026-11-04-req-07-08-22-collisions.md): where the role send, reply modes and time limits hit today's delivery, cold-wake and watchdog code; 6 open rulings.
- [2026-11-04 daemon 6.5 GB peak is the anomalies fold](2026-11-04-daemon-6.5gb-peak-is-anomalies-fold.md): post-bounce peak was an unscoped `pij anomalies` decoding all 4.18M spine rows (pre-#114, every call); node show and spine events too; indexed per-seat fix.
- [2026-11-06 s05 role send shape](2026-11-06-s05-role-send-shape.md): how role requests are stored, routed, queued, woken and drained (PR #129); seams for s06/s12; ddocs-set and #122-flake traps.

<!-- pij:generated -->
- [2026-11-06 Stacked PRs after a squash merge, shared-build waits and stale PM messages](2026-11-06-stacked-prs-after-a-squash-merge-shared-build-waits-and-stal.dd.md): Open when a stacked PR's base was squash-merged, a niced build is slow, or a PM ask may already be done (git, build, process, 129, 133)
- [2026-11-08 C32 caller binding and atomic audits; C31 steering; tooling without node\_modules](2026-11-08-c32-caller-binding-and-atomic-audits-c31.dd.md): Open before any identity-dependent effect \(answer, close, withdraw, notice\), editing the native Copilot receiver's steering, or freezing a PR without node\_modules (c32, delivery, steering, audit, plan-212, plan-214)
- [2026-11-08 Evidence first: store queries that settled F116, F120, F113 and F124](2026-11-08-evidence-first-store-queries-that.dd.md): Open when a delivery looks downgraded, held or refused: read the store and the extension log before touching code (delivery, diagnosis, copilot, receipts)
- [2026-11-08 Memory turn and compaction evidence as built \(F124, F130\)](2026-11-08-memory-turn-and-compaction-evidence-as.dd.md): Open before changing the memory turn, the compaction wake, the Copilot receiver's discard logic, or anything that asks whether a compact ran (memory-turn, compaction, receiver, f124, f130)
- [2026-11-09 Reply correlation: which routes carry in\_reply\_to \(F154 evidence\)](2026-11-09-reply-correlation-which-routes-carry-in.dd.md): Open before touching reply-answers-request \(plan 214 AC6\), the native pij\_send tool, or diagnosing role requests left unanswered (f154, replies, role-requests, copilot, shim)
- [2026-11-10 Answered requests looked unanswered: answered\_at was invisible \(F176\)](2026-11-10-answered-requests-looked-unanswered.dd.md): Open when someone reports role requests stuck delivered though replied, before touching pij requests output, or the in\_reply\_to receipt note (f176, role-requests, replies, diagnosis, receipts)
<!-- /pij:generated -->
