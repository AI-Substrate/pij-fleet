# Prompt-cache cost: why idle seats cost so much, and the 1-hour fix

A Claude seat on Copilot keeps its prompt cache for **5 minutes**. When a pij message wakes a seat that has been
idle for longer, the seat rebuilds its whole context (typically 200–450k tokens) as a cache write. A cache write
costs 1.25× the input price, against 0.05× for a cache read. In the week of 11–17 Nov 2026 those rebuilds were
about **45% of the fleet's model cost**. OMP seats can use a 1-hour cache today (below); Copilot CLI seats cannot.

Found 2026-11-18 by `pij-amazing-takin`, which investigated a usage blowout across Copilot and OMP seats.
`pij-continued-bolvar`'s Claude-era RCA (Sep 2026) found the same pattern for Claude Code seats.

## The cost of one call

Price each call's token classes separately. Opus 5.5 at OpenRouter list prices, per million tokens:

| Class | Price | Note |
|---|---|---|
| Fresh input | $4.00 | Tokens after the last cache breakpoint |
| Cache read | $0.20 | 0.05× input |
| Cache write, 5-minute | $5.00 | 1.25× input |
| Cache write, 1-hour | $8.00 | 2× input |
| Output | $20.00 | Includes hidden thinking tokens |

- **Each cache read refreshes the cache's expiry.** Calls less than the TTL apart therefore keep a seat warm
  indefinitely.
- **A gap longer than the TTL is a cold wake.** The next call writes the whole context again: a 300k context
  costs about $1.50 on a 5-minute cache, against $0.06 to read it.
- **GPT models** (gpt-6.1-sol and others) cache differently: there is no write premium, and Copilot records a
  1800-second TTL. The cold-wake penalty is much smaller.

## The week of 11–17 Nov 2026

Copilot CLI and OMP seats in the busiest projects, priced as above (the method is under *Reproduce*):

| | Cost |
|---|---|
| Total | ≈ $9.9k |
| Cold rebuilds (full cache writes) | ≈ $4.5k |
| Turns opened by another seat's message | ≈ 70% |
| Turns opened by pij nudges (watchdog, idle reminders, memory turns) | ≈ 3% |

- **The cost was concentrated in a few seats.** It sat in the project-manager (PM) seats under one prime. Each
  Copilot Opus PM cost $400–530 and received 900–2,000 normal (waking) pij messages that week.
- **FYIs were not the cause.** FYIs never wake a seat: they ride on the seat's next turn, and the `fyis` table
  records each as settled via `hook:*` or `message:*`.
- **Rebuilds came after idle gaps.** About 93% of rebuild cost came after a 5–60 minute gap since the seat's
  previous call on the same model.
- **Copilot hides the gap.** Copilot makes a small gpt-5.4-nano call just before each Opus call, so measured
  against the previous call of any model, the gap looks short and the rebuild looks like a cache miss.

## Copilot CLI: an adaptive TTL experiment, no user setting

Each Copilot `session.usage_record` carries `cacheTtlSeconds`, `cachePolicyTtlSeconds` and, inside the
experiment, `promptCacheTtlReason` and `promptCacheCreditsBucket`. The public Copilot SDK event schema documents
these as an "adaptive Anthropic prompt-cache policy" experiment.

- **Until 16 Nov it was 5 minutes everywhere.** Every Claude call recorded 300 s. Copilot also bills cache
  writes at $5/M, the 5-minute rate.
- **Since then, some calls get 1 hour.** Main-conversation calls (not subagents) get 3600 s in long runs.
  - 1 hour appears only with reason `l1_hot` or `l1_lapsed`, in sessions past about 200 AI credits.
  - About 40% of `l1_hot` calls get it. The split is assigned server-side per model session, and the client
    has no control over it.
  - Long-lived PM and prime seats got 1 hour on 50–84% of calls; most short-lived workers got 0%.
- **Your settings don't decide it.** Effort level, CLI version and turn initiator correlate with it but don't
  determine it. Subagent calls are always 5 minutes.
- **The runtime's env var can't turn 1 hour on.** Setting `ANTHROPIC_ADAPTIVE_PROMPT_CACHE_TTL` to any value we
  tried (`true`, `false`, `1`, `3600`, `hybrid`, `treatment`) switched the adaptive policy *off*: calls lost
  their reason and stayed at 5 minutes. `COPILOT_TEST_ADAPTIVE_PROMPT_CACHE_MINUTE_MS` did not end `warmup`.
  (tmux test sessions, 2026-11-18.)
- **No user-facing option exists yet.** The open request is
  [github/copilot-cli#5220](https://github.com/github/copilot-cli/issues/5220). VS Code's
  `github.copilot.chat.anthropic.promptCaching.extendedTtl` setting doesn't affect Copilot CLI.

### Routes to a longer TTL for Copilot seats

| Route | Status |
|---|---|
| A Copilot extension that rewrites requests | **Not possible.** The SDK's `CopilotRequestHandler` (experimental) can rewrite every outbound model request, but only an SDK *host* can register it, through `ClientModelOptions.requestHandler`, and it applies to the whole process. `joinSession()` extensions can't. |
| pij hosts Copilot seats through the SDK | Possible. It means replacing the interactive CLI pane with a pij-owned SDK host that sets `requestHandler` to add `ttl:"1h"`. |
| A localhost proxy via `COPILOT_PROVIDER_*` | Possible but fragile. It routes around Copilot's own login. |
| Experiment overrides (`COPILOT_EXPERIMENTS`, `COPILOT_EXP_*`) | Untested. |
| Keep busy Copilot seats from idling past 5 minutes, or batch messages to them | Works today. |

## OMP: use the 1-hour cache (shipped)

OMP sends `cache_control: {type: "ephemeral", ttl: "1h"}` on Anthropic Messages requests only when **both** of
these are set:

1. **Retention:** `providers.cacheRetention: long` in `~/.omp/agent/config.yml`. This is set by `just link`, one of
   pij's managed OMP settings. `just omp-doctor` (and `just doctor`) report it missing or stale, and `just unlink`
   resets it only while it still holds pij's value. OMP's own default is `auto`. The env var
   `PI_CACHE_RETENTION=long` works too.
2. **The model's compat flag:** `compat.supportsLongCacheRetention: true` on the model in `.omp/models.yml`. The
   repo sets it for Copilot's claude-sonnet-5.5, claude-opus-5 and claude-opus-5.5.

Watch the flag's name. `supportsLongPromptCacheRetention` is a different flag, read by OMP's OpenAI and Bedrock
paths; the Anthropic path ignores it.

Copilot's `/v1/messages` endpoint honours the 1-hour TTL from OMP. Receipts from 2026-11-18, using
`omp -p --no-extensions`:

| Call | Cache read | Cache write | `cttl` |
|---|---|---|---|
| Sonnet 5.5, first call | 0 | 14,249 | `ephemeral1h` |
| Same folder, after 6+ minutes idle | 13,935 | 309 | `ephemeral1h` |
| Opus 5.5, config setting only (no env var) | 0 | 14,244 | `ephemeral1h` |

Running seats pick it up on restart or `/reload`. Check any OMP session file: `usage.cttl` should show
`ephemeral1h`.

**The simulated saving is about $3.2k (32%) for the week.** That replays every Claude call with a 1-hour cache:
rebuilds after 5–60 minutes become reads, and every write is priced at 2× instead of 1.25×. The saving splits
into Copilot −40% (if it had 1 hour) and OMP −19%. It is an estimate: it assumes the context was unchanged
across the gap.

OMP's `providers.cacheWarming: idle`, the default, is meant to refresh 5-minute entries between runs. No warming
calls appear in OMP transcripts or logs, and OMP still had 430 rebuilds after 5–30 minute gaps that week. Don't
rely on it.

## Reproduce

Unisphere (`unisphere`) folds every harness's native transcripts into Parquet tables.
DuckDB recipes then turn them into tables:

```sh
unisphere prep --target "$T" --harness copilot-cli --harness oh-my-pi \
  --modified-since 2026-11-11T00:00:00Z --threads 4 \
  --max-record-bytes 67108864 --max-batch-bytes 67108864   # some Copilot records exceed the 16 MiB default
unisphere prep recipe daily-tokens --target "$T" | duckdb -csv
```

- **Pricing.** The recipes return token classes only. Price them by joining a per-model table (OpenRouter's
  `/api/v1/models` gives `prompt`, `completion` and `input_cache_read`; take the 1-hour write as 2× input).
- **Repo attribution.** Copilot sessions carry no project; take `cwd` from each
  `~/.copilot/session-state/<id>/workspace.yaml`.
- **Seat names.** Join `pij fleet-report`'s `tables/seats.jsonl` (`harness_sessions` → `seat`).
- **What opened each turn.** Copilot records pij deliveries as human prompts with no sender. Re-run prep with
  `--include-content` and classify `triggers.content_head`.
- **Normal vs FYI messages.** Use pij's store read-only: `delivery:<seat>` rows in `jobs` are normal messages,
  and rows in `fyis` are FYIs.

## Gaps in our own tooling

- **`pij fleet-report` under-prices Copilot seats about 4×.** It misses most of Copilot's cache counters (52k
  calls with only 26M cache reads in one project), and it assumes OMP calls without a TTL have a 1-hour cache.
- **Unisphere's `idle-wakes` recipe misses cold wakes.** It measures the gap from the previous call of *any*
  model, so the nano call before each Opus call hides the idle period. It also defaults OMP calls without
  `cttl` to a 1-hour cache.
- **Copilot hides pij message senders.** pij messages into Copilot seats are recorded as human prompts with no
  sender metadata, so a turn's trigger reads as `human`.
