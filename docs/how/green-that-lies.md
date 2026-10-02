# Green that lies — eleven ways a gate reports green without proving the claim

> **Why this file exists.** `docs/difficulties.md` has cited this file by wiki-link and by numbered
> mechanism since 2026-07-25, and the target has never existed in this repository. It lived in
> one seat's private memory. A committed document citing a target nobody can reach is worse than
> no citation: **a dangling link reads as coverage**, and a reader concludes the matter is
> documented and stops. Landed by `pij-reasonable-dove` (s099) on `pij-continuing-ermine`'s
> ruling. Companion: [`computed-but-unconsulted-signals.md`](./computed-but-unconsulted-signals.md).

**Treat "the tests pass" as a claim to verify, not a result to accept.** Observed first as five
distinct mechanisms in one session (2026-07-25), extended to eleven by 2026-08-02. Each is a
different mechanism with the same outcome: a gate is GREEN and does not prove what a reader
assumes.

This file is the *complement* to the fleet's **fail-first bar**, which governs criteria that
**cannot fail**. Most of what follows are criteria that **can** fail, label cleanly as behavioural,
and still prove nothing.

> **Deliberately not linked.** `docs/how/fleet/fail-first.md` and
> `docs/how/fleet/partition-categories.md` exist only on the unmerged `docs/fleet-live-findings`
> branch (PR #168), kept open as living documents. Linking them from main would create exactly the
> dangling reference this file was landed to fix. Read them with
> `git show origin/docs/fleet-live-findings:docs/how/fleet/fail-first.md` until they land.

---

## 1 · The gate doesn't run the code

`tsconfig.json` excludes `**/*.test.ts`, so `just typecheck` never sees test files. A
required-param change compiled clean while two `loop.test.ts` callers passed the old arity —
surfaced only as a runtime `TypeError` under vitest. It also let a dead import survive.

A criterion can be genuinely behavioural, genuinely capable of failing, and live in a file the
gate does not read.

## 2 · The mutation didn't apply

A mutation harness silently stopped applying after edits moved its anchor text. **A mutation that
does not apply reads as a PASS.** Caught only because the harness printed `MUTATION DID NOT
APPLY` — always print and check that line.

The fleet's `mutate.mjs` makes this structural rather than a human step: **exit 2 means the
mutation did not happen.** Prefer the exit code to the discipline.

## 3 · The mutation tests the wrong thing

A mutant named for *"mark self-injection before the gate"* actually inserted the mark **after** the
gate returned `held`, so it never exercised the behaviour its name claimed — and its survival was
being cited as evidence. **A mislabelled mutant is worse than no mutant.**

## 4 · The mutation kills for the wrong reason

Removing a *required* TypeScript argument makes tests die at a runtime type error. That proves the
argument is required, **not** that the gate is semantically doing its job — the actual race test
was not even among the failures.

Fix: add **behaviour-preserving** mutants that pass valid inputs and neuter the logic.

## 4b · The mutation nobody would think to make

*(Fourth mode, named 2026-08-08 from s093's finding; the first three are all the mutation being
**defective**, this one is a **correct** mutation that never gets attempted.)*

A reviewer hand-mutating naturally attacks the code **under test** — the guard — which is why a
first round can correctly go red and still miss the hole. **Nobody hand-edits a table in a
different file to attack a test that merely reads it.** s093's vacuous "valence pin" was only
exposed when adding `to` to core's `BOOLEAN_FLAGS` became cheap enough to try.

**Cheapness changed the outcome, not rigour** — so the fix is tooling, not discipline.

Pair this with §2: **exit 2 and exit 1 are opposite sides of one seam.** Exit 2 asks whether the
mutation *happened*; exit 1 asks whether the test could *perceive* it. Only exit 1 is a verdict on
a **test** — the one artifact in this pipeline that otherwise never gets one.

## 5 · The test passes only because of ambient environment

A canary test asserted error wording that differed on CI (no tmux pane → different branch). Passed
locally, failed in CI. **A test whose outcome depends on ambient state is not pinning behaviour.**

The inverse also bites: a criterion can fail pre-fix and pass post-fix for reasons unrelated to the
fix, so you record a genuine failure output and are satisfied by it.

## 6 · The guard is still PRESENT but no longer LOAD-BEARING

After a sibling stream rewrites the surrounding file, `grep` finds your guard and the suite is
green — but the guard may have become unreachable while its test keeps passing for another reason.
**Still-present and still-load-bearing are different claims.**

**Fix: re-run the Dim-0 revert on the combined/rebased tree**, not just the tree you developed
against. If deleting the guard no longer turns its test RED, the protection is gone. Verified
2026-07-25 (s070/s069): the exempt guard still went red after s069 rewrote `daemon.ts`.

> **This is the fleet-scale hazard.** A fail-first proof is established against your own tree,
> **pre-merge**, and convergence can invalidate it with nothing re-running it. The fleet's
> partition guidance (§5, on the branch above) states that a partition cannot partition a
> composition root — and its categories 3, 4, 5 and 6 are the exact index of at-risk proofs, which
> neither document claims to be. **Discharge the fail-first bar at authoring time AND again on the
> rebased tree.**

## 7 · The test asserts the absence of something that was never going to happen

A suppression test is **vacuous by default**. Two live instances in s070: a close-clobber test
staged the descriptor as already `dissolved`, where the registry *already* refuses stale writes —
it passed with the fix reverted. Caught only by reverting to check.

**Require a control test for every suppression fix**, with byte-identical setup, proving the
behaviour really fires without the fix. Stronger still (fail-first §3, branch above): prove the
absence is **selective** — on the same event, one watcher receives exactly one notice while another
receives none. An unpaired absence assertion proves nothing about which member caused it.

## 8 · The fakes pass where the production adapter crashes

PR #48 wrapped ports with `{...rawPorts, sendText}`. Spread copies only OWN properties, and
`DaemonTmux` is a class with prototype methods — so production got a ports object with `sendText`
and nothing else. **Every daemon tick crashed** (`ports.now is not a function`): the 2026-07-25
fleet-wide delivery outage. All tests green, because fakes are plain objects and spread fine.

Fix (`067b4a1`): `Object.assign(Object.create(Object.getPrototypeOf(raw)), raw, {...})`. **Rule:
never spread a class instance, and at least one gate must exercise the REAL adapter class.**

## 9 · The ENFORCEMENT test reads as a proof but is a text proxy

s071's write-law enforcer (`core/registry-write-law.test.ts`) greps source for raw descriptor
writes. Probed by injecting synthetic violations, it caught an explicit `field:` key and a
brand-new file, but **missed ES6 shorthand** (`{...latest, closeIntent}` — the idiomatic form of
the very thing guarded) and accepted a *payload value* `"cli"` as a declared authority.

Worse, its raw-write allowlist is FILE-level and lists `cli.ts`, `core/cli.ts`, `daemon.ts`,
`core/session.ts`, `core/daemon/loop.ts` — so **none of the five incidents it cites would have been
caught by it.**

**A guard that reads as complete is more dangerous than no guard.** Say *"tripwire, not proof"* in
its header, and keep the real protection in the default path (there: `write()` merges by default).

## 10 · The override that fires on every correct case

> A control whose override fires on every correct case is training its user to ignore it.

Worse than a crying-wolf detector, because the flag still **looks** like diligence while being
hollowed out. The mechanism: **the safe use and the dangerous use are typographically identical** —
`--repin` on a correct cross-worktree pin and on a genuinely wrong one are the same keystrokes.

Live instances: `baton grant --repin` (E-PIN resolves HEAD in the *granter's* repo, so every
legitimate sibling-worktree grant demands the override); `pij close --force`; `pij canary` timing
out against healthy peers, which trains seats to skip canary-verify entirely.

**The mitigation is a design move, not a discipline move.** Make the override **structurally unable
to reach the dangerous case**, as `--assume-dead` does: it overrides `uncertain` and cannot
override a genuine `live`, so the safe and dangerous keystrokes are not the same keystrokes — the
dangerous one does not exist. Corollary: if an override *must* reach the dangerous case, fix the
false demand so correct cases stop requiring it, rather than teaching people to click through.

## 11 · The guard is TOTAL OVER A HARDCODED LIST, so new members can never fail it

`harness/scripts/pij-skill-check.sh` asserts *"every CLI verb has a home in the skill"* — while
iterating a **literal verb list in the script**. Shipping `pij chore` left it undocumented in
`skills/pij/SKILL.md` and the check stayed green, because what it enumerates is *the verbs it
already knows*, not the CLI's real surface. It was also **not wired into `self-check` /
`harness checks`**, so it never ran in the gate at all — red with 10 pre-existing failures on
`origin/main` the whole time.

Same shape one layer over: `PA_VERB_CLASSIFICATION` (`core/orchestration/pa-capability.ts`) claims
to be total over CLI verbs, and nothing fails when a new verb family is unclassified — only the
docstring says it must be.

**The tell**: a check whose subject is *"all X"* but whose implementation is a literal list of X.
It is honest about what it enumerates and silent about what it omits, and the omission is exactly
the new thing you just built.

**Fix**: derive the list from the authority (the CLI's dispatch branches) so it is self-maintaining;
at minimum add the new member AND injection-prove the guard bites (remove the row → expect `✗`,
restore → expect `✓`). Also check the gate is **wired in**: a red-but-unwired check reads as
coverage.

---

## Bonus, same family, different surface

An assertion can be **too strict** and fail randomly: nine `pij-id` regexes required ≥2 segments
while the generator deliberately emits single-segment ship-namespace ids, breaking unrelated PRs at
random.

## How to apply

When a worker reports green, ask what the gate actually **executed**:

- Does the typecheck include the file?
- Did the mutation apply — did you *print* it, or check for exit 2?
- Does the mutant's name match what it edits?
- Would it die for the *intended* reason?
- Does the outcome depend on ambient tmux / HOME / network?
- Is the guard still load-bearing on the **rebased** tree, not just yours?

**A vacuous test is invisible to reading by construction — it is what a correct test looks like.**
Adding another reviewer buys nothing; only execution against a mutant separates them. If you catch
yourself proposing another review round to fix a vacuity problem, run a mutant instead.

**Related doctrine.** [`computed-but-unconsulted-signals.md`](./computed-but-unconsulted-signals.md)
is the sibling one layer down — a signal computed and wired to nothing; this file is *enforced, but
only over a frozen subset*. Two rules referenced by the original filing are not yet landed and are
stated here rather than linked, so nothing dangles: **own the whole gate** — when `harness checks`
is red, fix it even if pre-existing or outside your delegation, don't hand it back; and **the
instrument must separate the states** — before reporting that two states differ, confirm your probe
can tell them apart (`.get()` collapses an absent key into a null value, which is how an
absent-vs-null finding was filed using an instrument that could not see the difference).
