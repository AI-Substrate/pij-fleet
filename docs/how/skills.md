# How: skills

Skills are reusable prompt packs (`SKILL.md` + references) that agents discover
and invoke. Where they live differs by agent, and it is **easy to get wrong** —
the fact below is verified against disk reality, do not paraphrase it.

## The skills model — the shared store (verified)

> Non-Claude agents (Copilot, Codex, OMP) read skills from **`~/.agents/skills/`**
> (installed via `npx skills`; manifest `~/.agents/.skill-lock.json`). Claude's
> skills live at **`~/.claude/skills/`**, which **symlinks into** the shared
> `~/.agents/skills/` store.

Verify it yourself:

```bash
ls ~/.agents/skills/                 # the shared store (the-flow, flow-pair, …)
head ~/.agents/.skill-lock.json      # manifest, "version": 3
ls -la ~/.claude/skills/             # entries are symlinks → ../../.agents/skills/<name>
```

So there is **one** physical store (`~/.agents/skills/`) tracked by **one**
manifest (`~/.agents/.skill-lock.json`); Claude just sees it through symlinks.
Installing a skill machine-wide updates the shared store and the per-agent
symlink bridges in one pass — you don't install a skill separately per agent.

## Install recipes

| Recipe | Scope | What it does |
|--------|-------|--------------|
| `just pij-skill-link-global` | Machine-wide, canonical checkout only | Symlinks `skills/pij` to `~/.agents/skills/pij`, so the live skill always tracks this repo. Run by `just install`. |
| `just pij-skill-install` | Alias | Same as `pij-skill-link-global`. |

Other skills (for example an SDD pipeline skill) install with
`npx skills` directly; use symlink mode
(no `--copy`) so a live skill tracks its source.

## Which skills matter for cold start

For a fresh agent getting productive in pij, the three that matter most:

- **`the-flow`** — the SDD pipeline front door (see [`workflow.md`](workflow.md)).
- **`pij`** — the unified router front door (routes: pair · delegate · agent · peer · ops · skill);
  pairing is `/pij pair`. (The old `/flow-pair` skill was removed; saying "flow-pair" still routes here.)
- **flow-pair engine** — *not* an installed skill, but the orchestrator/worker/reviewer delegation
  **engine** (`skills/flow-pair/lib`) behind `/pij pair` (CLI, ledger, schemas, prompt-lab; see
  [`flow-pair.md`](flow-pair.md)).
- **`eng-harness-flow`** — the engineering-harness loop (boot / checks / improve;
  see [`build.md`](build.md) and
  [`.harness/engineering-harness.md`](../../.harness/engineering-harness.md)).

## See also

- [`workflow.md`](workflow.md) — how the-flow + flow-pair fit together.
- [`build.md`](build.md) — the build/gate harness the eng-harness skill drives.
