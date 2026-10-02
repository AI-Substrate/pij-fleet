# Historical exemplar — canary record

> Labeled run-01 history, not a template. Ids and nonce are intentionally
> preserved. Use [`../templates/stream-brief.md`](../templates/stream-brief.md)
> and [`../rituals/kickoff.md`](../rituals/kickoff.md) for current operation.
> Its old spawn flags and session columns are historical evidence, not current rs grammar. Current native spawn has no `--layout`; shim tail/list filters do not reproduce the old transcript/registry. Runtime canon now lives under `crates/testkit/fixtures/golden/api/`; this July receipt is not new cutover proof.

# Canary record — s017 (pij-1gx33y5)
**Spawner**: the prime (pij-1bovprr) · **Date**: 2026-07-10 · **Verdict**: PASS (3/3 legs; leg c closed ~13:00Z at brief delivery after the slate hold)

| Leg | Evidence | Result |
|-----|----------|--------|
| (a) Round-trip | Challenge sent 2026-07-10 ~12:31Z with nonce `S017-4482`; reply arrived as a daemon-injected turn: `canary-ack nonce=S017-4482 model=claude-fable-5 id=pij-1gx33y5` | PASS |
| (b) Identity (mechanical) | `pij sessions` row: `pij-1gx33y5 · claude · 3d3f5d33-97ef-41b8-b7f8-dcd2cdb56ba0 · bound · parent pij-1bovprr` (boundModel column empty — known gap); fallback probe per protocol: tmux pane %903 footer read `pij-1gx33y5 • Fable 5`, and the pane showed the peer composing the ack with `model=claude-fable-5` | PASS |
| (c) Input reliability | Brief-pointer send (~12:57Z, post-hold) landed; daemon-injected `brief-ack` received ~13:00Z incl. a relayed Jordan ruling — input path proven | PASS |

**Context**: spawn `pij spawn --harness claude --layout window` → pane %903, window renamed `s017-config`. Peer status: canaried, brief HELD per Jordan's slate ruling.

## The rule this story paid for

**Write the record at pass time.** The canary itself passed; the first cold audit
still failed because the nonce existed only in transcript. The record above was
written retroactively. Current operation reverses that order: file first, claim
second. Source: vendored war story 1.
