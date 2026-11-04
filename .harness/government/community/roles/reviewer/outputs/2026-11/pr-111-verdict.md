reviewer: block (head b13f5b3a433e3b410a8e376ca0252cb67358bbab)

Reviewed PR #111, plan 192 / s14. The packet pinned ed41a254612d47432349656831e7804baed302c0; the checkout and PR advanced during orientation to b13f5b3a433e3b410a8e376ca0252cb67358bbab. I reviewed the original diff and the sole follow-on change to D-192-3. Both findings below apply to both heads. No product files changed.

## Blocking findings

1. **P2: The typing-gate triage calls three shipped features missing.** `docs/plans/192-prd-register-move/assets/evidence/triage.md:12,32` keeps old req-0004 and req-0030 as PRD 002 req-03 because idle expiry and self-echo exemption supposedly do not exist. `docs/project/prds/002-pij-base.dd.json:107` additionally calls fresh per-send observation missing. Today's `crates/harnesses/src/interaction.rs` already implements self-injection tracking and exemption at lines 348-445, fresh capture at lines 486-609, and bounded edit-recency expiry at lines 573-589. Regression tests explicitly cover parked-draft expiry (827, 1204), self-echo (877, 1311), preservation of a human hold (913), and a fresh boundary overriding stale observations (979). Publishing this as work left to build defeats AC2/AC3 and sends future work toward reimplementing existing behavior. Re-triage both old ids against these tests: drop met behavior, or keep only a specifically demonstrated unmet remainder, and update the row, counts and rendered evidence together.

2. **P2: The Claude socket row revives an already-settled default.** `docs/plans/192-prd-register-move/assets/evidence/triage.md:10` and `docs/project/prds/002-pij-base.dd.json:128` say the socket path is off unless `--accept-inbound` is passed. The PRD's q-01 at lines 300-303 therefore asks for an operator ruling and prescribes an off default. But `crates/cli/src/main.rs:2858-2864` already defaults Claude spawns to inbound acceptance and documents the existing operator ruling. Its flag help at lines 306-314 and tests `claude_spawn_accepts_inbound_by_default` (3830) and `no_accept_inbound_withdraws_the_claude_default` (3887) pin the behavior. Direct HTTP callers explicitly sending false are a separate boundary, documented and tested in `crates/daemon/src/http/tests.rs:4427-4492`. Correct the triage, note and question to distinguish the CLI default from direct HTTP; retain only any remaining demonstrated socket/receipt gap rather than reopening the decided CLI policy.

## Evidence and coverage

| Check | Result |
|---|---|
| `python3 docs/plans/192-prd-register-move/assets/evidence/check.py <private-checkout>` | Passed: frozen sha e05661c8; 54 rows, 40 open; 40 verdicts; 23 kept, 12 met, 5 obsolete; 20 target rows; no missing, extra, orphan or duplicate rows; zero scrub hits and shared eight-word runs. This proves accounting and lexical checks, not semantic triage correctness. |
| Reviewer scratch mutation probe | Passed five cases: clean baseline exits 0; a planted source-derived eight-word run counts 1 and exits 1; deleting an open-row verdict exits 1; duplicating a verdict exits 1; restoring the clean inputs exits 0. Only counts were printed. The scratch copy was removed. |
| `ddocs validate docs/project/prds/001-pij-community.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs validate docs/project/prds/002-pij-base.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs validate docs/project/roadmaps/001-pij-community.dd.json` | Passed, 0 errors / 0 warnings. |
| `ddocs build <file> --check` for those three documents, plan 192, its backpressure and phase-1 tasks | All six passed without drift or writes. |
| Main-checkout `node_modules/.bin/biome check .` against this checkout | Passed, 218 files; one informational CLI/schema-version mismatch, no errors or warnings. |
| Main-checkout `node_modules/.bin/tsx harness/scripts/local-path-check.ts` | Passed. |
| Main-checkout `node_modules/.bin/tsx harness/scripts/lockfile-allowlist.ts` | Passed, 223 allowlisted sources. |
| `harness boot --json` | Advisory not-ready: this review worktree has no linked Node dependencies. No install or dependency repair performed. |
| `harness checks --quick --json` | Failed five local tooling stages: local-paths, typecheck, lockfile, lint and test. Copilot native contracts passed; Rust and smoke were skipped. Local default Biome is too old for this config; the explicit main-checkout Biome passes. No claim of a green local full gate. |
| `gh pr checks 111`; `gh run view 41765163791` | All four jobs passed at exact current head b13f5b3a433e3b410a8e376ca0252cb67358bbab: check (22), check (24), rust-lint and rust-test. The superseded ed41a254 run was cancelled, not passed. |

Minimum current-code spot checks completed, independently of the triage's wording:

| Verdict | Old id | Current-code evidence |
|---|---|---|
| Kept | req-0012 -> req-07 | `crates/cli/src/main.rs:274-323` and `prepare_spawn_request` expose cwd but no first-task field. |
| Kept | req-0014 -> req-14 | `tsconfig.json:19` excludes `**/*.test.ts`. |
| Kept | req-0026 -> req-10 | `crates/harnesses/src/spawn.rs:142-146` constructs the child launcher with seat/spawn identity, not daemon address/state-directory variables. |
| Dropped as met | req-0009 | `harness/scripts/claude-session-start-pij.sh:23-29,47-55` adopts the real Claude session; daemon startup installs it in `crates/cli/src/main.rs:2340-2342`. |
| Dropped as met | req-0015 | `crates/daemon/src/registration.rs:139-150,169-235` observes external-host/session evidence for paneless admission; `crates/daemon/src/http/shim.rs:905-944` supports waiting; `.omp/extensions/pij/adapters/generation-router.ts:370-418` wires ambient registration. |
| Dropped as met | req-0027 | `crates/cli/src/main.rs:354-358,2173-2199` reads files/stdin; `.omp/extensions/pij/adapters/generation-router.ts:508-526` reads the shim literal body through its injected port. The review's own quoted-heredoc send also received a queued native receipt. |
| Dropped as obsolete | req-0011 | `.omp/extensions/pij/core/generation-routing.ts:494-513` refuses the retired generation instead of falling back. |
| Dropped as obsolete | req-0043 | The same rs-only routing decision and `docs/how/pij-rs-api.md:3` remove the dual-generation premise. |

PRD 002 has testable done_when fields, all rows planned, and req-20 is could/planned with C17's no-unearned-binding constraint. C11/C12/C15/C17 were read. C15's prose says 42 open rows; the frozen source and the checker both establish 40, so the PR's count is justified.

PRD 001 preserves all four main changelog entries and roadmap 001 all three. s13 and s14 are complete; req-25 is shipped with C15's fresh-triage wording. All amended workshop source paths resolve. AGENTS.md and bootstrap.md name PRD 002; the operational AGENTS/skills/harness scan finds no stale `prd/base-prd` pointer.

D-192-1 matches the formatter exclusions and preserved plan 191 canary evidence. D-192-2 matches `prepare_spawn_request`'s linked-worktree refusal and that canary's clone workaround. D-192-3 correctly describes tombstone-only close (`crates/cli/src/main.rs:499`); its additional observed live-card disagreement is reported as an incident, not claimed fixed by this PR.

Security is a separate role's verdict. There is no changed product loop, poll, timer, hook, sweep, status line or launch; the one-off evidence checker has no steady-state or per-seat cost.

## Handoff and harness feedback

Please fix the two factual triage blockers, regenerate the affected documents and counts, and request a review at the replacement head. Green schema/copy/count checks and CI do not establish that the kept backlog is still unmet.

magicWand: each kept row carries a current-code and regression-test receipt before it can be accepted as missing work. Difficulty captured as reviewer DL-001: the deterministic accounting guard passes even when a row contradicts implemented behavior. Keep the counts-only positive-control mutation probe as a self-test for the evidence checker; do not extend its success claim to semantic correctness.
