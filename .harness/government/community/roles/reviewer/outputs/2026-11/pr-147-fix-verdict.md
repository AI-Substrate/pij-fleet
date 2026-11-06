# PR #147 forwarding repair review

## Verdict

**pass-with-notes**, exact head `5c8ff97a8cb021e4f862afdde137713c50658c05`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/147#issuecomment-6682289411

F001, F002 and N001 are closed under the documented tmux-only fast-send scope. No remaining blocking correctness finding in the complete seven-file `29488ca..5c8ff97` repair delta.

Request: `325693b633ea1aca-0000000000000113`, role reviewer/147, reply wake.

## Closure

| Finding | Disposition and evidence |
|---|---|
| F001: flag values misread as recipients | Closed. JSON tables now cover native send/global value flags and are held equal to clap, including a new-flag negative control. `scanSend` consumes separate/equals values and honors `--`. All three original order cases and the role-looking msg-id seat control are adopted in the real shim CI tests, along with every native value option. |
| F002: inherited ID bypassed paneless registration | Closed. A real send is fast-forwarded only with TMUX_PANE; PIJ_SESSION_ID no longer enables that branch. Both positional role and --to inherited-ID fixtures remain routed. The unchanged admission bridge registers the observed native host and overrides inherited identity. Its 67-test module passes Node CI, including ambient-admission, sender/caller forwarding and terminal refusal controls. |
| N001: leading globals missed the fast path | Closed. `findVerb` consumes --json and the clap-bound global value flags for classification, while the actual child still receives the original argv. Full-shim CI exercises --json, separate --state-dir and inline --addr ahead of community/send/help commands. |

The original oracle cases are now part of stronger whole-shim recorder CI, not merely assertions against a reimplemented selector. The new clap arity guard prevents the exact hand-maintained-list drift that caused F001.

## Coupled changes

- `resolvePijRs` chooses an executable only from an absolute PATH entry and spawns that absolute candidate. The relative-entry negative and named missing-binary/127 controls pass CI.
- Native acting-seat resolution now sends `caller_context()` in `IdentityRequest`, preserving the daemon's harness-session checks rather than sending only a seat/pane assertion. This change is typed through the existing shared context.
- Caller environment, stdin and argv remain unchanged at the direct spawn; no shell, output re-renderer or JSON wrapper is introduced. Kept positional-seat, shim-only flags and non-rs-generation paths retain their documented routing.
- The paneless controls prove non-forwarding and retention of the old admission path; they are not a claim that the old HTTP parser now accepts arbitrary native-only syntax. No such parser expansion is included.

Independent binary/PATH security grading and performance's latency receipt remain separately owned. No latency or live sender impersonation test was performed by this reviewer.

## Exact-head proof

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42197742059

All four jobs pass at the reviewed SHA. Rust summary: **1,978 passed, five skipped**. Published JUnit confirms all three clap contract tests and all six shim_forward tests, including the arity inventory and relative-PATH negative.

Node 22 CI logs show `generation-router.test.ts` **67 passed**, within **2,198 passed/six skipped** overall. The retained observed-host admission component is therefore exercised, not inferred merely from the Rust boundary test.

The remaining explicit local CI gap was `--` termination and help-looking body data. A bounded oracle executed the unchanged production classifier/helper functions against their real JSON metadata under:

`nice -n 19 taskpolicy -b node <reviewer-output>/2026-11/pr-147-repair-selector.cjs <review-root>`

**22 cases passed**, including the original F001/F002/N001 cases, role/native-to/seat/legacy controls, separate/equals values, paneless inherited-ID forms, global prefixes, option terminators and opaque help data. This pure selector check launches neither tsx nor a daemon and does not claim end-to-end sender attribution. No Rust build, install or dependency link was needed.

Final GitHub/worktree pin matches the requested head and PR is CLEAN. Worktree is unchanged; no product edits, commits/pushes or deployment by this reviewer.

## Cost and handover

For eligible calls the fast path remains one node start plus one native child, now spawned by resolved absolute path; no per-tick work or daemon launch site is added. Broader send parsing is bounded by the argv length. Prefix-global community calls now avoid the old second Node/tsx path.

No correctness change is owed for the three findings. Keep the documented tmux-only identity boundary and clap-derived value tables. Independent security/performance conclusions are not replaced by this correctness pass.

**magicWand outcome:** original counterexamples became full-shim CI controls, and value-option arity is now tied to clap with a growth negative. The small independent oracle fills explicit terminator/help-data cases without paying for the routed runtime.

**difficulty:** none in this round required the earlier slow full-router attempt; the pure selector checks completed cleanly at background QoS.

Role memory was written through the live verb using a real body file, at writer commit `59040f1f4f250e3da2b158c109d2bd248b171a2b`, note `2026-11-06-pr-147-forwarding-repair-5c8ff97.dd.json`. No role note or generated INDEX block was hand-edited.
