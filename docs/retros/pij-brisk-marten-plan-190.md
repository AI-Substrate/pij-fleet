# pij-brisk-marten: the community writer and `pij directory add` (plan 190, phase 2)

**magicWand:** a `just private-daemon` recipe that boots a private daemon (temp
HOME and state, private `-L` tmux server and port) with a scratch seat already
bound to its pane, and prints the env to call it with. Every cost receipt in
this repo rebuilds that by hand. Plan 187's `private-replay.sh` and this plan's
`private-run.sh` are two copies, and this one took four tries to get a seat the
daemon would accept.

**What shipped:**
- A Rust port of `government-root.sh`, ddocs resolved once at daemon start
  (`PIJ_DDOCS`, else PATH; on ping as `data.ddocs` and in `just doctor`), and the
  daemon pid on ping.
- The community writer: it commits only its own paths with `git add` then
  `git commit --only` in the standing governance worktree (C8), refuses foreign
  edits on those paths, serialises per worktree, waits out a held `index.lock`
  and never forces it, and carries the `pij/*` schema copies (C9).
- `pij directory add` (flags or `--file`) and `--batch`, all or nothing; the
  store pointer (migration 0031) with a rebuild from the branch head.
- Cost on a private daemon: 0 at rest, 9 launches per add, 23 per eight-role
  batch (ddocs is per document).

**difficulties:**
- My own seat's `TMUX_PANE` leaked into the private daemon run, and pane
  identity wins over `PIJ_SESSION_ID`, so every call resolved to a seat the
  private store had never seen. Then `/v1/register` refused a claim with no bind
  evidence. The script now unsets the ambient identity variables and binds the
  scratch seat to the private server's pane.
- macOS `/tmp` is `/private/tmp`: the caller's cwd arrives canonical while a
  project's repo may be recorded as typed, so a prefix match silently found no
  project. Found only by the end-to-end run; fixed by comparing canonical paths.
- This worktree has no `node_modules`, and the packet forbids
  `just worktree-deps` and `npm install`, so the TS half of the gates
  (typecheck, lint, vitest, smoke, the TS skill-coverage test) cannot run here.
- `ddocs add --mint` returns the minted id at `data.value.id`, not `data.id`;
  a script that guessed the shape added a task, then crashed before wiring it.
- The plan said roadmap s02 "goes to shipped"; the roadmap schema's step state
  is `complete`. The schema won.
