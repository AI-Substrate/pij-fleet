# PR #147 native-forwarding review

## Verdict

**block**, exact head `29488cad374621621303fe1153c695784cb914ce`.

Published: https://github.com/AI-Substrate/pij-fleet/pull/147#issuecomment-6681743124

Two P2 forwarding/identity boundaries remain incorrect despite green CI. N001 is a nonblocking fast-path coverage note. Review is scoped to plan 213's diff and the send/identity paths it changes. Fresh worktree remains clean and frozen; the PR is CLEAN and has left draft.

Request: `325693b633ea1aca-000000000000009d`, role reviewer/147, reply wake.

## F001 (P2): value-taking send flags change whether a valid native call is forwarded

`harness/scripts/pij-cli.cjs:11-22,40-44` omits `--body`, `--from` and `--msg-id` from `SEND_VALUE_FLAGS`. Their values are therefore mistaken for the positional recipient, causing an early return before the real `--to` or `role:` target is reached.

The production selector returned false for all of these valid native forms with a pane identity:

```
send --body hello --to role:probe
send --from pij-sender role:probe hello
send --msg-id id-147 role:probe hello
```

They consequently fall into the TS/HTTP shim parser instead of the native parser added by this PR. The old daemon `parse_send` rejects the unsupported body/from/msg-id flags (and treats --to as an unsupported broadcast form), so native-supported role sends still fail depending on flag order. The opposite error also occurs: a seat send with `--msg-id role:fake` is classified as a role send.

**Fix:** consume every value-taking native send option before identifying the recipient, including separate and equals forms, and respect option/positional boundaries. Tie the arity inventory or its test to clap rather than checking only six selected flag names. Add role-send tests with those flags before the recipient/--to and kept-path controls whose opaque values look like role addresses. Preserve argv exactly; do not rewrite values to make the selector pass.

## F002 (P2): a paneless inherited PIJ_SESSION_ID bypasses observed host registration

At `pij-cli.cjs:39`, a nonempty exported seat ID is treated as sufficient to forward a paneless send. This is precisely an input the existing routed path deliberately does not trust.

The production selector returned false for a paneless Copilot native-session fixture without a seat ID, but true for the same child-native-session fixture with `PIJ_SESSION_ID=pij-parent-fixture`. There is still no TMUX_PANE.

The subsequent source trace is unambiguous:

- The retained TS router, `generation-router.ts:381-458`, performs ambient native-host registration whenever there is no pane, then overrides an inherited seat ID with the daemon-observed child's ID.
- The new shortcut skips that route entirely.
- Native `identity::request_from_environment` sends only seat/pane claims for acting-seat resolution. `caller_context` does not attach an observed pid/start tuple.
- With no pane, daemon `resolve_seat` returns the live descriptor named by the asserted ID. The role-send request and `from` are then built with that parent ID, not the native child the old route would have registered.

This is a normal inherited-environment regression, not a claim that the exported ID is verified. It can attribute a child's role request to the parent instead of merely making the child slower.

**Fix:** keep all paneless sends on the verified routed/registration path, even when PIJ_SESSION_ID is present, unless the native shortcut first performs the equivalent observed-host admission and uses its result. Do not use presence of an inherited environment value as proof. Add a paneless-child regression with a live parent ID plus a distinct native session, proving correct child sender attribution and unchanged parent binding.

**Proof grade:** the selector decision was executed directly from the unchanged checked-in function with positive/negative controls. The wrong-sender consequence is a source trace through the native resolver and daemon; no live daemon impersonation/send was performed. Security's independent caller-identity verdict remains separately owned.

## N001 (P3): leading --json misses the claimed cheap path

The CJS selector assumes argv[0] is the verb. The existing TS entry explicitly supports `pij --json claims` by locating/reordering the verb, but this form still loads tsx before later delegating natively. Thus the fast latency receipt applies to verb-first calls, not every supported community invocation.

Consider locating the verb past supported global options for classification only while passing the original argv unchanged, and add a prefix-JSON recorder case. This is not an additional merge blocker.

## What otherwise checks out

- The four current native-owned community verbs share the JSON list; the clap test detects newly unlisted and stale listed verbs.
- Spawn uses argv arrays and inherited stdio rather than a shell or response renderer. The ordinary help/flag/stdin/output/status contract passes CI.
- Positional recipient/text and --to/--body forms are mutually exclusive as intended; control-body validation also checks positional text.
- Positional seat sends, genuinely paneless sends without any ID, and explicit legacy generation controls are covered, but the inherited-ID and opaque-value cases above are missing.
- The TS native-command route retains configured state/addr and caller environment; error text now names the actual command.

Binary-resolution/PATH trust and the independent security verdict are not regraded here. Performance's reported interleaved timing is retained as author evidence: 0.546→0.085 seconds for claim help, 0.639→0.115 for claims. No latency measurement was repeated by this reviewer.

## CI and local proof

Run: https://github.com/AI-Substrate/pij-fleet/actions/runs/42191982701

Exact reviewed SHA, all four jobs successful: Node 22, Node 24, rust-lint and rust-test. Rust summary: **1,976 passed, five skipped**. Published JUnit confirms both clap contract tests and all five shim_forward tests.

The current flag test enumerates flags for the four unconditional community verbs, but send forwarding has only selected examples and a six-flag presence check. Its kept-path negatives cover --reason's value, not the omitted native value options or inherited native identity.

For these CI gaps, local checks were niced/background:

- A full-shim recording-binary positive control passed argv/stdin unchanged. Its first kept-path continuation exceeded a ten-second bound while entering the routed path; this reviewer-harness timeout is not attributed to the PR, and no completion of the remaining full-shim cases is claimed. A process check found no surviving recorder-specific routed child.
- A smaller oracle then executed only the actual production selector in an isolated VM, with its real JSON verb list, no launch/router branches and no daemon. Normal role/--to, seat and legacy controls passed. All three missed flag-order cases, the role-looking msg-id misclassification and inherited-ID paneless bypass reproduced.

The deterministic selector oracle is archived alongside this report as `pr-147-selector-oracle.cjs`; its executed session-file copy used `nice -n 19 taskpolicy -b node <oracle> <review-root>`. No Rust build, dependency link/install, product edit or deployment occurred.

## Handover and harness gift

Close F001/F002 with focused forwarding/identity controls, then provide a frozen repair head and CI. The current head's green tests do not prove these untested boundaries.

**magicWand:** test forwarding as a grammar-and-identity decision, not just byte equality after the happy branch was chosen. Drive send option arity from clap and include a native child inheriting its parent's exported seat ID.

**difficulty:** executing the full kept path was unnecessary and slow under low QoS. The pure production-selector oracle provided deterministic branch evidence without launching tsx or a daemon; keep that evidence grade distinct from a full sender-attribution test.

Role memory was written through the live verb using a real body file, at writer commit `83394aa1aeb7fbe792cc5b541395b3872243c60b`, note `2026-11-06-pr-147-shim-forwarding-boundaries-29488ca.dd.json`. No role note or generated INDEX block was hand-edited.

**Separate coordination F-item:** the attempted security handoff was refused twice by the live directory: `role:security/147` said the role had one holder/no instances and prescribed `role:security`; that address then reported two live holders and prescribed an instance or seat. No stale seat was guessed and neither message was delivered. PM must route the F002 pointer to the assigned security reviewer or reconcile the directory. This is live routing friction, not another finding against PR #147.
