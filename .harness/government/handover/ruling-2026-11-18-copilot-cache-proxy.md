# Prime rulings: Copilot cache proxy (PRD 004, blueprint 003, workstream 004)

**From** pij-outstanding-silkworm (pij-fleet prime) · **To** pij-amazing-takin · **Date** 2026-11-18
**Answers** takin's brief: `~/.copilot/session-state/b3a977ab-e482-4534-8554-61d23750fe83/files/prime-brief-copilot-cache-proxy.md`

## Rulings

| # | Question | Ruling | Why |
|---|---|---|---|
| 1 | `model-cost` role? | **Yes: a new specialist role**, `model-cost` (long-lived, one holder). Scope: prompt-cache and model-cost economics, `fleet-report` pricing, the proxy, cost receipts. **takin holds it.** Create it with `pij directory add model-cost --why ...`; never hand-edit. | Two C28 signals: the same finding twice (bolvar, Sep; takin, Nov), and knowledge worth keeping. No existing role covers it: `performance` is about process and CPU cost, not model spend. |
| 2 | Who holds pm-004? | **takin, for workstream 004 only.** | Its steps *are* model-cost work, so this is one domain, not two. pm-001's holder (otter) is no longer live, and Jordan wants fewer seats. If a second workstream lands on model-cost, split pm-004 off to its own seat. |
| 3 | Requirement 2: credentials | **Default: pure pass-through** (the caller's own token is forwarded unchanged). **Named-account mode only with a per-install key**: a stable random secret in a `0600` file that the proxy creates, which a request must carry, or it gets refused. Never per-boot (seats can't follow a bounce); never in argv. **Jordan has the final say**: ask him with this recommendation. | A loopback TCP port is reachable by **every** account on the Mac, not only Jordan's. C33 draws the trust boundary at the user account, and #229's S229-001 blocked on exactly this cross-account reach. Without a key, named-account mode lets another local account make model calls billed to Jordan. Pass-through has no such issue: no token, no call. |
| 4 | Workstream 004: standalone or under 002? | **Standalone** workstream 004, with its own PRD 004. Its rows cite the PRD 002 rows they touch (spawn, daemon, doctor). | It's a new capability with its own requirements and acceptance, not a slice of the base register. |
| 5 | Daemon process or sidecar? | **Sidecar, same binary**: a `pij-rs cache-proxy` subcommand under its own LaunchAgent, on its own loopback port. The daemon and `just doctor` only report its health (and `ping` its counters). | Over two days the daemon was bounced 8 times. In-process, every bounce would drop in-flight model calls for every Copilot Claude seat, which breaks "never break a seat". Using the same binary means no new build artifact, and Defender's process exclusion for `~/.local/share/pij/bin/pij-rs` already covers it. |

## Also ruled

- **Ship the OMP fix now, separately.** The verified OMP change on `fix/omp-copilot-1h-cache` (`compat.supportsLongCacheRetention`, `providers.cacheRetention: long`) goes as its own small PR. It doesn't wait for PRD 004.
- **Process:** PRD 004 first (`ddocs` on main, then `ddocs validate` and `ddocs build`; see `docs/project/README.md`), then blueprint 003, then workstream 004. No coder lane before the workstream exists.
- **Build economy (C12, Jordan 2026-11-11):** one build per PR, by its build owner; reviewers reuse it and never build their own copy. One active lane at a time on this workstream. Close seats between lanes (memory, close, kill pane, check the process is gone).
- **Cost row is mandatory:** the proxy is a listener plus `gh` launches, so it needs a performance verdict, with spawn-counter receipts for "no launch per request; one `gh` per account per proxy lifetime, plus one per 401".
- **Security verdict is mandatory:** credential resolution, plus an unauthenticated (or keyed) loopback listener.
- **Forbidden paths:** `the-flow.json`, `the-flow.md`, and the governance ledger dirs (writer only).
