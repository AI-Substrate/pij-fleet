# Historical exemplar — baton grant log

> Labeled run-01 excerpts, not live state. They show the four paths a useful
> book must represent: first grant, self-grant, reclaim, and breach.

## First grant

- 2026-07-10T13:02:21Z · push-main · pij-vut1pp (overseer) · GRANTED — 018 docs snapshot commit+push, docs-only. Pre-grant check: baton free, no competing git activity known; verified working tree is at 31792e4. First-ever grant; the book binding its own designer is the intended dogfood.

## Self-grant

- 2026-07-11T04:43Z · dotnet · pij-vsa9qj (s017) · RETURNED (long-hold closed at phase close per terms — clean: fleet quiet since gate-done) → immediately SELF-GRANTED to pij-uec99o (o-prime) for the s017-0003 verification gate → `harness checks --quick` GREEN (build 9.9s, csharp 14.6s, config 0.8s) → RETURNED 04:44Z. The book bound its own keeper; verification reproduced the stream's gate claim independently.

## Silent-holder reclaim

- 2026-07-11T01:47:00Z · push-main · pij-vut1pp (overseer) · RECLAIMED by the o-prime — holder died in the machine restart before returning grant #2; purpose was COMPLETED before death (evidence: `f26d3fd` "docs(018): o-prime run-01 snapshot 2" on main). First live exercise of the silent-holder reclaim rule.

## Breach

- 2026-07-11T04:29:38Z · dotnet · pij-lewt29 (s021) · BREACH (benign, self-reported) — `harness boot --no-build` ran the quick gate incl. xUnit (~15s, green) during s017's long-hold, WITHOUT a grant. No collision occurred. The paved boot path omitted the baton warning; the stream self-reported and the orient was fixed.

## Other rules the lived record paid for

**Dissolved state and seat identity are registry truths.** One closed stream was
resurrected by queued events; another seat spent hours receiving orchestrator
messages inside a stood-down research persona. A descriptor must distinguish
dissolved from crashed, and sends name a role when a seat can host more than one
persona. Sources: vendored war stories 3 and 8.

**Compile at every yield.** Fences did not save siblings when one stream left an
orphan test and another yielded a compile error. Work stays in scratch until it
builds; the non-owner stops and routes an urgent owner-fix. Source: vendored war
story 9.
