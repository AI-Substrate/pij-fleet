# skill — run an installed skill in a peer

> Route module — sibling-blind. Knows only this job; composition is the dispatch's job.
> Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: hand ONE skill invocation (`/validate-v2`, `/thesis`, any installed skill) to a fresh peer and get its output pushed back to you. No pack, no I/O schemas — **the prompt is the whole contract**.

**Preconditions**: verified delivery ownership per § C1, a reachable rs daemon, a known report target, and the requested skill actually installed in the peer's host. Never install/symlink global resources implicitly. `pij agent spawn/report` is unported; this route uses an ordinary bounded peer packet, not a hidden agent-pack fallback.

## Native peer and packet

```bash
pij-rs spawn --harness <h> --model <exact-model> --cwd <absolute-repo> --parent <your-seat> --json
pij canary <peer> --expect-model <exact-model> --json
pij dispatch <peer> --packet <absolute-packet-path> --wait --json
```

Persist the packet before dispatch. It supplies the skill invocation, allowed/forbidden paths, report target and output contract. Native spawn has no `--prompt`, `--once` or legacy layout flags; there is no automatic report-and-close guarantee. The peer sends its result pointer through supported `pij send` to the recorded target; let the pushed turn wake you (§ C7). Model catalog per § C4; actual placement per § C5. Reuse/compact healthy peers (§ C3); rs close tombstones only.

## Prompt recipe

Three parts, one sentence each where possible:

1. **Invocation** — name the skill as typed and give its args: `Invoke your /validate-v2 skill with: --artifact <path>`. Use absolute paths; add `cd <repo> first` when the target repo ≠ your cwd. Tell it to **fail loudly if the skill is missing** — never improvise the skill from memory.
2. **Response shape** — "respond per the skill's own output format", or narrower ("verdict + findings table only").
3. **Report shape** — short outputs ride inline: `{"summary":"<one line>","output":"<full text>"}`. Long outputs (review tables, dossiers): have the peer write a file and report the path (pointer discipline — dispatch invariant 2): `{"summary","verdict","path"}`. `summary` follows § C10: the verdict/action first, no restatement of the ask.

Example packet text (ordinary task content, not a captured runtime result):

```text
Invoke your /thesis skill with args: thing <absolute-file>; work only in <absolute-repo>.
If the skill is unavailable, report that instead of improvising.
Respond in the skill's own format; write large output to <owned-output-path>.
Send the result pointer to <parent-seat> using pij send; do not use pij agent report.
```

## Failure modes

| Symptom | Meaning / move |
|---|---|
| Caller unresolved | Respect extension ownership or exact-pane admission (§ C1); never start a competing inbox registration |
| Skill missing or output improvised | Report the missing prerequisite; do not install global links or accept memory as execution |
| Report never arrives | Trust supported push; bounded host transcript/pane inspection only for a broken transport, not native event tail |
| Wrong model/runtime | Re-derive from the host catalog/footer and canary; `pij models` refuses |
