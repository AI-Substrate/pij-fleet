# Wave B state (15:20)

tags: wave-b, state, handover
hook: Plans 194/195/196 pushed; marten codes s04; heron: docs PR, then jobs 7 (195 schemas) and 8 (194 templates); s11 coder waits on disk.

- Wave A merged: #110 (s13), #111 (s14, PRD 002), #113 (ddocs sensor), #114 (s02 writer), #116 (C19 scrub). Prod daemon bounced on 6ccd4d8 (pid 97253). Issue #117: the writer's TOCTOU residual.
- Plans: 194 s03 plan/194-role-templates; 195 s04 plan/195-assign-lineage (coder pij-brisk-marten in ~/pi-hacking/fleet-190-community-writer, adding AC13-15 for dogfood F2/F3/F4); 196 s11 plan/196-claims-delivery (no coder yet; disk at 25 GB, asked prime to free space; spawn sharing marten's CARGO_TARGET_DIR).
- Heron (docs-system) queue: a docs PR (D-190-5 + dogfood frictions as D-WB-n + a "don't pre-validate a role" procedure line), job 7, job 8. Packet: outputs/2026-11/001-wave-b-docs-system-packet.md.
- Rulings this wave: C22 (prompted core roles), C23 (pij delivered verb). Dogfood F1 (no pij project registered) is with the prime.
- Practice: freeze a head and say "cancel earlier push instructions" in the same message; give each verdict seat a fresh worktree per head, remove worktrees only after every seat using them has reported; retire seats by pij close then kill <pid>.
