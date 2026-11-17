# pij-previous-firefly — fleet metro map as a pij skill tool (plan 227)

**magicWand:** `harness checks` that runs correctly under the fleet's own `nice -n 19` rule. Run that way, the
gate fails `harness/scripts/shared-build.test.ts:306`, because the recipe's `nice -n 19` stacks on the outer one and
the test sees niceness 20. The rule and the gate contradict each other, so every coder who follows the rule gets a
red `test` stage it must explain away.

**What shipped:**
- `pij fleet-map init|generate|open`. It is a node skill tool at `skills/pij/tools/fleet-map/` with no
  dependencies, and the node shim runs it before pij-rs or tsx load. There is no daemon change.
- By default the structure lives in the governance worktree and the output goes to the main checkout's git-ignored
  `scratch/fleet-map/`.
- Data comes from pij first (`list`, `directory`, `outputs`). PR comment verdicts fill only the roles pij lacks.
- The POC page is kept as a template, with out-of-order merges fixed.

**difficulties:**
- A mutation run that disabled the skill-folder guard let its own refusal test write into the real skill folder.
  The test ran the tool from the repo, so the guarded folder was the source tree. The test now runs a temporary
  copy of the tool. Tests of a "never write X" guard should target a copy of X.
- Biome lints HTML, so an imported page fails `just lint` on the POC's style. The template folder is excluded
  rather than rewritten.
- `pij outputs --head` matches only the full sha (a short one returns nothing), and `pij directory` and
  `pij outputs` refuse outside a registered repo unless given `--project`. Both refusals come back as `ok:false`
  envelopes, so the tool checks `ok`, not only the exit code.
