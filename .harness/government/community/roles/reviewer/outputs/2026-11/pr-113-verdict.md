reviewer: block (head 2b328dc6b71788fee466a0a8c1e0b7fc9720df1d)

## A) Verdict

**Block: P2 silent success after a document-tree read failure.**

## B) Summary

The normal validation, render-drift and unavailable-tool paths work. However, the filesystem adapter turns every directory-read error into an empty directory, so the gate can report success without checking an entire required tree. This contradicts AC1's every-document guarantee.

This supersedes the earlier verdict for `75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb`. PM-001 confirmed the checkout move and reassigned this frozen head. The complete delta is six documentation files, +41/-2: PRD 001 req-27 for C18, plus plan AC6/bp-0006 and generated faces. It introduces no new correctness finding and does not repair F001.

The sensor blob is `3047b271cd26b79ad84835465f833f4105445a9e` at both heads; source, tests, config, CI and wiring are unchanged. The earlier 14 CLI mutation probes were extracted from the literal earlier SHA, not an ambient `HEAD`, and remain valid evidence for this identical implementation. Current-head verification freshly passed all six project/example documents, both changed plan/pressure documents and their generated faces. Exact-head CI is green.

## C) Checklist

- Seven focused unit tests pass; they test decisions and injected ports, not constants.
- Clean six-document validation passes; broken links and rendered drift fail; restoration passes.
- Missing ddocs exits 2 with UNAVAILABLE. Relative and invalid absolute overrides refuse without PATH fallback.
- CI warn mode prints its warning annotation when ddocs is missing, but still exits 1 on a real validation failure when ddocs is present.
- The new stage is in both checks and self-check, and AGENTS.md names it.
- The CI diff adds only the documented sensor step and comments; it adds no install or permissions. C20 explicitly accepts unavailable-tool warnings in CI.
- Discovery errors do not fail closed: F001 below.
- No source edits, installs, dependency links, commits, pushes or daemon operations were performed by this reviewer.

## D) Findings

| ID | Priority | Location | Finding |
|---|---|---|---|
| F001 | P2, blocking | `harness/scripts/ddocs-check.ts:126-127` | The blanket `catch { return []; }` silently drops unreadable directories; remaining documents let the overall gate pass. |

## E) Reproduction and fix

In an isolated archive of earlier head `75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb`, using real ddocs 0.1.0 and the byte-identical sensor still present at this head:

1. Remove required section `why` from `docs/project/prds/001-pij-community.dd.json`. The sensor exits **1**, reporting the missing section and render drift. This is the positive control.
2. Make scratch `docs/project` unreadable with `chmod 000`. Run the same sensor with the same invalid PRD still present. It exits **0**, printing `ddocs-check: 3 document(s) validate clean and their .dd.md are current (ddocs via PATH).` It reports no inaccessible-tree warning.
3. Restore permissions without fixing the PRD. The sensor exits **1** again.
4. Restore the original document bytes. All **6** documents pass, exit **0**.

`listDirectory` catches EACCES as though the tree were genuinely empty. Since the healthy community examples still provide three documents, the global zero-document guard cannot detect the loss. The same suppression applies to other read failures and nested directories.

**Required fix:** propagate discovery failures as a named nonzero sensor result, with the repo-relative directory and filesystem error. Do not silently substitute an empty list. Add a regression that combines one failing directory read with another nonempty healthy root; the current tests model absence as an empty list and never exercise this adapter failure. Keep the existing clean/invalid/unavailable behavior.

## F) Acceptance coverage

| Criterion | Result | Evidence |
|---|---|---|
| AC1, every document and no silent pass | **Fail** | F001; unreadable project root passes despite a known-invalid PRD |
| AC2, executable resolution | Pass | Unit tests plus real exit-2 probes for absent, relative and bad absolute ddocs |
| AC3, integration | Pass by source inspection | checks stage, self-check recipe, AGENTS.md inventory and CI step |
| AC4, mutation proof | Pass for specified cases | Broken link and hand-edited Markdown fail; restored corpus passes |
| AC5, CI availability decision | Accepted ruling, not validation proof | C20; warn-mode missing-tool annotation, real failure still exit 1 |
| AC6, C18 recorded as req-27 | Pass | Planned requirement matches C18, names s03/s09/s10, has the source and one changelog row; PRD validates/renders clean, plan AC6 and bp-0006 validate/renders clean |

No separate guide or execution log is declared by this Simple plan. Its live evidence is `assets/evidence/fires.txt`; independent probe script and full JSON receipts are retained as `pr113-probes.mjs` and `pr113-probes.json` in the review session artifacts.

## G) Validation and limitations

Commands used main-checkout binaries without installing or linking packages:

| Command | Result |
|---|---|
| `vitest run --root "$PWD" harness/scripts/ddocs-check.test.ts` | Initial startup blocked: local config cannot resolve `vitest/config` |
| `vitest run --config <main-checkout>/vitest.config.ts --root "$PWD" harness/scripts/ddocs-check.test.ts` | **7 tests passed**; relevant source/config unchanged from assigned head |
| `node <session-artifacts>/pr113-probes.mjs` | **14 asserted CLI cases completed**, including the reproduced silent-pass defect; script archives the literal assigned SHA |
| `biome check .` with shared binary | Passed, 219 files; one schema-version informational diagnostic, no errors or warnings |
| `tsx harness/scripts/local-path-check.ts` | Passed |
| `tsx harness/scripts/lockfile-allowlist.ts` | Passed, 223 sources allowlisted |
| `tsx harness/scripts/ddocs-check.ts` at `2b328dc` | Fresh pass, all six current project/example documents validate and render clean |
| `ddocs validate --json` and `ddocs build --check --json` on changed `plan.dd.json` and `assets/backpressure.dd.json` | Fresh pass, both documents 0 errors/0 warnings and both rendered faces current |
| `biome check docs/project/prds/001-pij-community.dd.json` | Fresh pass |
| `just --dry-run ddocs-check --unavailable=warn` | Correct recipe expansion |
| `harness boot --json` | Not ready: missing worktree node_modules, as expected from the packet |
| `harness checks --quick --json` | **Failed six tooling stages**; copilot-native passed; Rust/smoke skipped |
| Earlier-head CI run `41766405307` | Cancelled after supersession; rust-lint passed, other three jobs cancelled |
| Exact-head CI run `41766527358` | **All four jobs passed:** check (22), check (24), rust-lint, rust-test |

The earlier local quick gate remains tooling-limited and no full local green gate is claimed. Unchanged code tests were not repeated for this documentation-only delta. No Rust build or live smoke was run by this reviewer. Security and cost verdicts belong to the separately assigned reviewers.

## H) Handover and harness feedback

Send F001 to the implementation owner, repair the adapter and regression, then obtain a verdict for the resulting exact head. No other correctness blocker was found.

**magicWand:** a permanent CLI regression for partial discovery failure, proving that another healthy tree cannot turn inaccessible documents into success.

**Difficulties:** DL-001, dependency-free review tooling needs shared config as well as a shared binary; DL-002, traversal failure is success-shaped; DL-003, an externally moved review checkout invalidates implicit HEAD provenance. All were captured through `harness observe`. The reusable probe now extracts the full packet SHA explicitly, so the evidence does not depend on checkout position.
