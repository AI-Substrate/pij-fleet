# PR #132 memory-note follow-up

**Closed at `2851637fd10be349d30329e0f49704eb12519f69`:** explicit generated-block ownership plus escaped metadata address both F004 cases. The adopted hand-written `.dd.md` row and linked-title round-trip tests pass in exact-head CI run 42150037898. Current verdict is in `pr-132-verdict.md`; this historical addendum is retained for provenance.

Source basis: `5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22`. This is a separate source-review finding for the unchanged note logic. It does not re-pin the posted verdict to the forthcoming head. No local execution or probes.

## F004, P2: generated-row detection can delete hand-written rows and duplicate generated ones

Locations: `crates/daemon/src/community/memory.rs:168-176,378-416`.

`index` removes every existing line that `generated` recognizes. Recognition is only a Markdown-shape heuristic: a `- [` bullet whose first `](` target ends in `.dd.md` and contains no slash. It neither checks that a matching pij note exists nor records that pij authored the row.

A previously hand-written line such as:

```markdown
- [Read first](historical.dd.md): keep this hand-written explanation
```

is therefore removed on the next memory add, even if it is not a generated row and no corresponding `historical.dd.json` is available to regenerate it. This violates preservation of existing hand-written INDEX lines.

The inverse also occurs for valid titles. Title `Read [delivery](flow)` is accepted by `note` and by the pij/memory schema. `line` does not escape its label and emits:

```markdown
- [2026-11-06 Read [delivery](flow)](2026-11-06-read-delivery-flow.dd.md): hook
```

On the next regeneration, `generated` examines the first target, `flow`, and treats this row as hand-written. It keeps the old row and appends the regenerated row for the same note. Every later memory add adds another duplicate, violating one line per note.

**Required repair:** establish reliable ownership of generated rows without discarding unknown hand-written lines, and escape/index titles so generated output round-trips through regeneration. Preserve the two cases in CI. The existing passing preservation test covers plain `.md` links, bare names and dotted rows, but not these inputs.

Proof grade: direct source/input trace, not a local reproduction. Existing exact-head CI remains green; no build/test/probe is requested under backoff.
