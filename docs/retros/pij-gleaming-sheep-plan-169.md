# pij-gleaming-sheep — plan 169 (Copilot HydraFusion models)

**magicWand:** a `PIJ_NATIVE_TRACE=<file>` switch in the Copilot extension that
records the live event shape the receiver actually saw (type, id, parent,
agent, interaction, fusion ids; no bodies). The cascade stall was invisible in
both the persisted `events.jsonl` (no ephemeral phase stream) and the
extension's stderr reports (only `completion-wait`, then silence); proving it
took a hand-written second extension.

**difficulties:**
- D-169-1: a private `COPILOT_HOME` has no installed runtime under `pkg/`, so
  the base launcher rejected `hydrafusion-max` with a stale static model list;
  the daemon's service-shell `PATH` also found a January `~/.local/bin/copilot`.
  Same symptom, two causes. Fixed for the canary with a runtime symlink and an
  absolute `--bin` wrapper.
- D-169-2: no first-class trace of the receiver's live view (the magic wand).
- `session.usage_record` is persisted-only, so the live parent chain always has
  gaps. Ordinary turns heal through the interactionId re-anchor; any future
  Copilot turn shape that changes interactionId mid-turn will stall the inbox
  the same way. The fusionId anchor covers HydraFusion only.
- `critique` never routed in six turns (four `single`, two `cascade`); its
  coverage is claimed by mechanism (same fusionId anchor), not observation.
- Load stayed above the C12 ceiling for most of the session; Rust checks waited.
