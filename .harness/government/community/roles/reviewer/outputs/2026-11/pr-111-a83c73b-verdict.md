reviewer: pass-with-notes (head a83c73b9fcaf4a1688e8139f60e420f403ca55cd)

Final scoped review of PR #111: only the F003 repair and count changes from 75102c5 were reopened. **F003 is closed; no correctness blocker remains.** The two earlier P2s remain closed and unchanged review evidence carries forward.

Old req-0005 and req-0022 are both kept as the new PRD 002 req-14, should/planned. The requirement is limited to live receiver-side proof for typed-pane harnesses; its done_when requires an exact body over 4 KB in the receiving transcript once, on the first attempt. The note correctly grades the existing paste/control-byte implementation as sender-side evidence, not live receipt proof. No transport rebuild or live canary is required in this documentation PR.

The previous 13 PRD rows are byte-equivalent as parsed records; only req-14 was added. Both source mappings and the evidence entry exist. Counts agree across triage, scrub output, plan receipts, changelogs and generated faces: **40 verdicts; 16 kept as 14 PRD rows; 19 met; 5 obsolete.** The seven-versus-nine introductory wording is also corrected.

| Validation | Result |
|---|---|
| `python3 docs/plans/192-prd-register-move/assets/evidence/check.py <private-checkout>` | Passed: 40/40, expected counts, no missing/extra/orphan/duplicate problems, zero scrub hits and shared eight-word runs. |
| `ddocs validate docs/project/prds/002-pij-base.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs validate docs/project/roadmaps/001-pij-community.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs build <file> --check` on the changed PRD, roadmap and plan | All three passed with no drift or writes. |
| Main-checkout `node_modules/.bin/biome check docs/project/prds/002-pij-base.dd.json docs/project/roadmaps/001-pij-community.dd.json` | Passed, two files, no fixes. |
| `gh run watch 41768606246 --exit-status --interval 20` | Passed all four jobs at this exact head: check (22), check (24), rust-lint and rust-test. |
| `harness boot --json`; `harness checks --quick --json` | Same no-install worktree limitation: boot not-ready; five local tooling stages fail, native Copilot contracts pass, Rust/smoke skipped. Not a green local full gate. |

The evidence checker did not change, so the six positive/negative mutation controls from the 75102c5 review still apply; only its final real-input accounting was rerun. Security's exact-head verdict is separate. This delta adds no product loop, recurring launch or per-seat cost.

**Note only:** the dependency-limited local composite gate is explicitly not claimed green; exact-head CI and targeted document checks are green. No additional correctness changes are requested.

Harness lesson retained in req-14: use receiver-side transcript evidence for real-delivery claims, not sender-side implementation as a substitute. Neither review worktree was modified; no install, commit, push or daemon action was performed.
