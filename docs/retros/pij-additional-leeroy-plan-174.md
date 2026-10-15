# pij-additional-leeroy: double-seat warning (plan 174)

**magicWand:** one fixture-only gate command that obeys the packet's resource
and isolation limits, stores every stage's full output, and reports prohibited
stages as NOT RUN rather than requiring an agent to reconstruct the gate.

**Harness improvement:** the incident is now a fixture-driven daemon test and
four script behavior tests. The notice brake, stray-record witness, startup
warning and red status line each have a mutation receipt. A FIFO synchronizes
the detached heal test with its actual log write; no new sleep-poll loop is
needed. The receipt index explicitly separates behavior changes that fail on
base from regression pins that already pass there.

**difficulties:**

| Severity | Observation | Disposition |
|---|---|---|
| high | The first script red run started at 1-minute load 42.56 despite the packet's ceiling. | Subsequent Cargo calls used a load-gated, niced, bounded launcher and local target; the final base-script red receipt was rerun below the ceiling. `stretch:` encode C12 in the canonical gate, not an agent's shell memory. |
| low | A status-line assertion rejected the word `double` anywhere in output; the fixture directory itself contained that word. | Assert the actual `⚠ double` segment instead. The final base-script red receipt uses that corrected test. |
| medium | The resumed worktree has no node_modules, and boot returns `ready:false`; the packet also forbids dependency writes. | Parent msg `0072` rejected dependency repair and accepted exact-head CI `check (22)`/`check (24)` instead of the local TS stages. The attempted quick gate remains recorded as failed, not rewritten as a pass. |
| medium | Full `harness checks` includes the live-tmux Driver smoke, prohibited by the packet. | Do not run it against shared services or call a quick gate a full pass. Record the precise scope and seek the parent's disposition. |
| medium | The handoff said code was done, but ACs, task assertions and execution receipts were still absent. | Verified the source and raw logs directly, persisted the logs beside the plan, and attached evidence through the installed `ddocs` writer before changing completion state. Local `ddocs` was unavailable and no dependencies were installed. |
| low | Two new daemon tests protect unchanged behavior and therefore pass on base. | Parent ruling `0078`: record each as "pin, green on base", never red, and prove each pin can fail. M5a (alter the not-registered text), M5b (accept a stale record's start) and M6 (resolved path reads records) each fail exactly their pin, then were restored. `stretch:` plan templates could ask for a mutation receipt for every regression pin up front. |

The original implementation ran under HydraFusion; GPT-6 Astra resumed the
delivery work after escalation, and Claude Opus 5.5 finished the pin mutations,
the Rust gate and delivery. Neither successful fixture tests nor CI are
claimed as live Claude UI or production-daemon proof.
