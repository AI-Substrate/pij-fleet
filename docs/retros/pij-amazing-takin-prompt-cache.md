# pij-amazing-takin — prompt-cache cost of idle seats, and OMP's 1-hour cache

**magicWand:** a `pij fleet-report` that prices Copilot and OMP seats correctly. It would read Copilot's cache
counters, take each call's native cache TTL, measure idle gaps against the previous call on the same model, and
split a seat's cost into normal messages, FYIs, nudges and human turns. This investigation needed Unisphere, raw
DuckDB, pij's store and hand-made prompt classification to get a number fleet-report should have printed.

**What shipped:**
- `docs/how/prompt-cache-cost.md`: how one call's cost is made up, the 11–17 Nov breakdown, Copilot's adaptive
  TTL experiment, and the routes to a longer TTL.
- `.omp/models.yml`: `compat.supportsLongCacheRetention: true` on Copilot's Claude models. With
  `providers.cacheRetention: long`, OMP seats get a 1-hour cache, confirmed by a cache hit after 6+ minutes idle.

**difficulties:**
- `pij fleet-report` priced one project at $1.7k against $7.8k from the raw counters, because it misses
  Copilot's cache-read counters.
- Copilot's nano call before each Opus call hides idle gaps, so a gap must be measured per model.
- OMP has two similarly named flags: `supportsLongPromptCacheRetention` (OpenAI and Bedrock) and
  `supportsLongCacheRetention` (Anthropic). The first, tried first, silently did nothing.
- Appending to `~/.omp/agent/config.yml`, which has no trailing newline, glued a key onto the previous line.
  Fixed at once. Edit YAML with a parser, not `>>`.
- `omp -p` crashes with `AgentBusyError` when extensions load; `--no-extensions` works.
