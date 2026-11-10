# pij-doubtful-lemur — end-of-turn pij blocks for Claude and Copilot (plan 226, lane B)

**magicWand:** a harness-level "show me how this hook is really called" probe.
Claude's MessageDisplay hook looked serial in the docs and in the prime's POC,
and only a timestamped wrapper on a throwaway seat showed that consecutive
flushes run concurrently. Every new hook surface (Copilot's onAgentStop,
Claude's MessageDisplay, OMP's renderer) needed a hand-written probe to learn
its ordering, its inputs and whether its output reaches the model. One reusable
probe per harness, logging invocation times, inputs and pids to a file, would
have turned each of those into a one-command check.

**What shipped:**
- Copilot: a `TurnEndTracker` over the native event stream (last root
  `assistant.message`, sub-agents ignored, reset on a prompt or abort, handed
  over once) and `onAgentStop` that hands a fenced reply to `/v1/turn-end`,
  shows the summary with `session.log`, and blocks for one follow-up turn only
  on failure and only when `stopHookActive` is false. Launches nothing.
- Claude Stop: the existing script makes its one `pij-rs` launch
  `hook claude-stop` when the input holds a pij fence; it runs the block, keeps
  the seat `working` when it gets its follow-up turn, and answers the hook.
- Claude MessageDisplay: a managed script that execs `pij-rs hook
  claude-display`, one process per flush, no daemon, flushes processed in index
  order.
- `pij-rs doctor claude-hooks` and `pij-rs claude-hooks --remove` over all five
  pij-managed hooks; `just doctor` and `just unlink` run them.

**difficulties:**
- D-226-4: Claude runs consecutive flushes' MessageDisplay hooks concurrently;
  the shell-and-jq first version raced and showed the block's tail raw.
- D-226-5: a cold worktree build took about 35 minutes at load 60 to 100;
  `taskpolicy -b` throttles IO too, so cargo sat at 0% CPU unpacking a crate.
  The shared target fixed it.
- D-226-6: Claude's folder-trust prompt defaults to "No, exit".
- Copilot's `onAgentStop` input carries no reply text; the text comes from the
  event stream, which arrived 9 to 159 ms before the hook on every stop, while
  the transcript file lagged (phase 1).
- The bash tool's guard refuses nested command substitution, so the display
  drivers became small Python scripts.
