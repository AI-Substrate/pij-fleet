# A prime's journey

You are a project's prime. Here is how a community grows, from an empty repo to pods you can start and stop with one command.

1. **Set up once.** `pij community init --why <text>` copies **each type's seed** (what a seat of that type is told first) into your governance, so your repo can edit its own seeds. It does not copy the type's **rules**: those stay pij's, the same for every project (Jordan, 2026-11-26). `--into <path>` only moves the pod templates it copies (`--templates <name>,...`) out to files you commit; the seeds still go to governance. pij copies nothing on its own; this is a step you take.
2. **Name roles as the work needs them.** `pij directory add <role> --type <type> --why <text> ...`. The type says how the seat lives (coder, reviewer, knowledge, steward...); the role says what it is for. `pij docs get types` lists the ten types.
3. **Start a seat in its role.** `pij-rs spawn --as role:<id> --set <name>=<value>`: the seat starts holding the role, with its first turn built (seed, role, parameters, your packet, memory index). Only `pij-rs` has this grammar; the `pij` shim refuses it. Preview the first turn with `pij directory show <role> --first-turn`.
4. **The second time you stand up the same group of seats, make it a pod.** `pij pod up <template> --set <name>=<value> [--session <tmux>]` starts every slot; `pij pod down <pod>` closes them through the template's close procedure, and a knowledge seat stays open. Start from a shipped template: `pij pod templates`, then `pij pod adopt <name> --into <path> --why <text>` gives you a copy you own. The shipped **live-call** template names its roles literally (`note-taker`, `researcher`...): adopt it, then rename its roles for your project (Jordan, 2026-11-26). Parameterised role names are not supported.
5. **Run the pod's procedures.** A template's open and close steps are procedures, kept in the pij skill's **pods** area (Jordan, 2026-11-26) and run with `pij proc`. `pij pod template-diff <path>` shows what changed upstream since your copy; nothing is applied for you.
6. **Turn what worked into the template.** When a pod ran well after you reshaped it, `pij pod export <pod> --as-template <path> --why <text>` writes it back out as it actually ran.
7. **Close cleanly.** `pij close <seat> --stop` asks a role holder to write memory first; a seat that holds no role stops at once.

Read next: `pij docs get concepts`, `pods`, `types`.
