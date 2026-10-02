# Kickoff — spawn or adopt one stream

Run steps in order. Artifacts make the process reconstructable; conversation does not.

## Steps 1–17

1. **Record the ruling.** Date the human's named work in the authoritative spine, as close to verbatim as possible.
2. **Allocate.** Scan ordinals; run `pij stream create --project <p> --slug <s> [--base <ref>] [--ordinal N]`.
   Its allocation reserves ordinal, branch, worktree and create-time base SHA; tombstones stay burned.
3. **Derive descriptive fences from actions.** Verify expected paths and scratch space.
   Record separate-branch overlap as merge risk (never a spawn block); name convergence.
4. **Add the roster row.** Status `preparing`, worktree/branch/base and UTC stamp BEFORE prose; polished notes beside stale rows fooled readers twice.
5. **Write the brief before spawning.** Instantiate the [stream brief](../templates/stream-brief.md)
   with ask, fences, worktree/branch/base, tree, prior art, cadence and provisional status.
6. **Construct before spawn.** Read `pij stream create` evidence/allocation journal;
   it owns worktree creation/resume and branch/base verification, replacing `git worktree add -b`.
   Shared-tree construction requires an explicit fallback ruling.
7. **Spawn into the worktree.** `pij-rs spawn --harness <h> --model <m> --effort <e> --cwd <absolute-worktree> --parent <o-prime-id> --name <stream-window> --json`. Native spawn has no legacy `--layout`/`--task`; use owned tmux placement after it reports the pane. Shim spawn refuses rather than falling back.
   Check the peer's actual `process.cwd()` and native tool root against the chosen worktree; a cwd flag or spawn receipt is not that observation.
8. **Observe the actual native binding result / pushed turn.** Native spawn waits by default; `--no-wait` is explicit admission, not readiness. Never poll a booting peer as a substitute for delivery.
9. **Verify placement.**
   Read `pij list --json`, `pij node show <id> --json`, the actual branch and tmux panes; folder, recorded parent, model and window must match the brief. List `--here` scopes to caller cwd, not every stream worktree; tree/link remain unported.
10. **Canary legs (a) and (b).** Run `pij canary <id> --expect-model <m>`;
    Complete canary leg (a) round-trip and leg (b) identity proof;
    record nonce-dispatch + defensive runtime evidence at pass time; (c) remains pending.
    Governor asserts `pij role <id> pm --json` after verifying the recorded parent. Spawn/adopt carries supported parent admission; existing-seat reparenting is a named gap, not a role side effect. Record rs parent and self/current-parent close authority, not inferred spawnedBy fallback.
11. **Deliver the brief by pointer as canary leg (c).** Run `pij dispatch <id>
    --packet <brief> --wait`; its first instruction is `/pij prime`. The seat
    runs the header's `pij ack <dispatch-id> --packet-sha <sha>` first; then close leg (c).
12. **Sync the spine; keep the structure tree live.** Fill id, `briefed`, stamp.
    Push every roster change to all streams; briefs name o-prime/sibling ids and windows.
13. **Report one hop up.** Use [`reports.md`](./reports.md); if this is a
    topless o-prime, the government record + human digest replaces numbering.
14. **Orient and preamble.** Stream invokes `/pij prime`, follows module-first journey, lands preamble report before planning mutation.
15. **Preserve before retirement.** Require PR merge or explicit abandonment evidence and preserve artifacts; then stand-down and authorized `pij close <id> --json`. Verify the tombstone receipt and active-roster exclusion; the process/pane remains untouched. Observe outstanding delivery to its real terminal outcome; no queue withdrawal/cancellation is introduced. Separately authorize terminal/worktree teardown, retire allocation/history without erasure, return/reclaim batons only with the observed `--lease-id`, and transplant findings.
16. **Diff manifest against descriptive fences.** At plan validation compare both
    ways: task paths outside the declared touch set and declared paths no task
    uses. Worktree-local additions are tell-not-ask (global invariant 11).
17. **Adoption variant.** Human-spawned peer skips 6–7 only; unknown provenance
    increases canary importance. Record verified rs parent and actual ownership; do not infer a spawnedBy authorization fallback.
    **Instantiate every stream-brief section, Orient stack included** (the first
    outside run freelanced away the levers); roster `ADOPTED`, provisional until human preamble.

## Canary

Canonical rs request/response and event examples live at `crates/testkit/fixtures/golden/api/governance-routes.json` and `governance-events.json`; [stable API](../../../../../docs/how/pij-rs-api.md) owns current grammar and named gaps. These are fixture examples, not new live captures or production/chainglass acceptance.

Write `<government-root>/canaries/s<ord>.md` while evidence is fresh; resolve the root via
[Governance branch — the rules](./bootstrap.md#governance-branch--the-rules):

| Leg | Mechanical proof |
|---|---|
| (a) round-trip | Send a nonce challenge; ack must arrive as a daemon-injected turn |
| (b) identity | Read registry/state for harness, model, effort, parent, and native session; if a field is absent/unbound, capture the pane footer as the explicit fallback — never accept bare self-assertion. An UNPINNED peer can only honestly self-report "default" (its args carry nothing to verify) — the prover of actual model/effort is YOUR footer/registry probe, so don't demand self-confidence the peer cannot have |
| (c) input reliability | A second send lands; use the brief-pointer send and require `brief-ack`. Adopting a mid-flight peer: legs (a)+(b) may be satisfied by the orientation exchange itself; record (c) as PENDING-on-brief-ack and close it in the record when the ack lands |

The first run did all three and still failed its audit because the record lived
only in transcript. **Pass-time file first, claim second.**

## Live deviations folded in

- Settle the item before spawn; otherwise a canaried peer idles on hold.
- Structure tree and scratch fence are fields, not afterthoughts.
- Human direct-go: deconfliction default per [`protocol.md`](../protocol.md) § Human rulings.
- Worktree-confined work is notify-only; synchronize at convergence or shared
  mutable resources ([`protocol.md`](../protocol.md) § Construction).

## Shared-tree fallback yield rule

Worktree-primary construction removes routine tree/index collisions. If a ruled
shared-tree fallback is unavoidable, fences still do not protect siblings' build
windows; two streams proved that in opposite directions within one hour.

1. Author uncompilable work in scratch; move it in-tree only when it builds.
2. Every pause, handoff, yield, and every **commit** includes the repo's
   compile/typecheck probe: a commit's transitive type closure must compile at
   checkout (run-01 nearly shipped referencing a sibling's untracked sources). Commits follow `00-routing.md` § C11 (Commit attribution).
3. A non-owner never repairs a sibling's broken file: stop, send an **urgent
   owner-fix** with the failing command and exact path.
4. The rule cuts upward and beyond compilers: the o-prime's own untracked
   CLI-generated file once redded a stream's FORMAT gate — ignore-list generated
   files (`.prettierignore` etc.); never hand-format them.
