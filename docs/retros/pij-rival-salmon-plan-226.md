# pij-rival-salmon — end-of-turn pij blocks, daemon and OMP (plan 226, lane A)

**magicWand:** a build lane that is fair to waiting seats. Under
`nice -n 19 taskpolicy -b` at load 55-150, my clippy of `pij_daemon` got 11 s
of CPU in 24 minutes (3%): background QoS holds every niced build to the
efficiency cores, and three seats' builds shared them. The rule stops one build
starving the machine; nothing stops the machine starving every build.

**What shipped:**
- `pij_core::turn_end`: finds the `pij` block that ends a reply (the last
  `` ```pij `` fence, closed, nothing after it), splits commands shell-style
  without expanding anything, allows `send`, `output` and `requests close` only,
  judges each route's answer, and renders the human's one line and the seat's
  follow-up (what ran, what was refused, what did not run).
- `POST /v1/turn-end`: validates every command with its verb's own parser
  (`parse_send`, output's `prepare`, `parse_close`, each now shared with its
  route), runs nothing if any fails, then runs them in order through the same
  handlers `pij send`, `pij output` and `pij requests close` reach, stopping at
  the first refusal. The run is its own task so a client that gives up cannot
  cut a block in half.
- OMP: `session_stop` hands the final text over only when it may end with a
  block; the result line is a `pij-turn-end` custom message drawn once idle;
  `decision: block` only when `stop_hook_active` is false; an assistant-text
  renderer hides the block, and a reply that ended in a tool call or an abort
  is redrawn raw. `just link` turns on `extensionRenderers.assistantText`
  through `omp config`, and `just omp-doctor` names it with the fix when off.

**difficulties:** D-226-1 (OMP `/reload` keeps an edited module; spawn refuses
a linked-worktree cwd), D-226-2 (no node_modules: biome via `npx` at the
lockfile's version, vitest through a stand-in), D-226-3 (cold cargo in a fresh
target).

**Found on the way:**
- A notice raised inside OMP's `session_stop` or the extension's `agent_end`
  never draws: the settle redraw drops it. A custom message posted once
  `ctx.isIdle()` does, and it stays.
- OMP's renderer sees a segment view: a text segment before a tool call does
  not include the call, and its `stopReason` reads `stop`. Only `message_end`
  knows, so it records the reply and asks for a redraw.
- OMP caps `continue: true` stop-hook continuations at 8 but never caps
  `decision: block`, so the loop guard has to be the extension's own.

**Round 1 review, and what it taught:**
- "The last ```pij line" is not a Markdown rule. A ```text block that quotes
  a pij example ends with the same four lines as a real block. Fences must be
  tracked from the start of the text, in the parser and in every renderer that
  hides the block (spoonbill F001).
- Validate-first is only as good as what the preflight checks. `parse_send`
  stops at syntax, and the send path's other pure rules (force needs a
  reason, role-only options) ran later, after earlier lines had sent. The fix
  pulls each rule out as a pure function, which both the send path and the
  preflight call, so the two can't drift (spoonbill F002).
- Text a seat or a route wrote, echoed back to a human or a seat, is caller
  text: fence it, every time (squirrel S212-N001). A renderer called on every
  frame needs a cheap path for the common case (ocelot P212-N001).
