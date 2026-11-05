# Daemon spawn attribution: registration, UTC offset and cargo

tags: cost, attribution, native-registration, utc-offset, cargo, measurement-limits
hook: date predates s04 and has a conditional forbidden per-tick/per-pending-seat path; the original 774/s cannot be partitioned into cargo and daemon.

Requested by pm-001 in message `a9b61371c0e5b381-0000000000000038`. Receipt `2026-11-06-plan195-at-rest.md` retains the original 09:12 window. All code references below are at **deployed merge `f327a7dd579658be89dc2eb1aa890744dd95bb78`**, not a moving main checkout.

## 1. `ps.registration-native-process`

Producer: `crates/daemon/src/registration.rs:1639-1662`, counter at **1647**, absolute `/bin/ps` at **1648**. One requested PID, `ppid/ucomm/args`; bounded output and one-second process timeout. The 5ms wait loop at `1692-1695` polls the already-launched child; it does not launch another ps each time.

Call path:

- Native extension `POST /v1/register`: `crates/daemon/src/http/mod.rs:953-965` -> `RegistrationService::register_native` at `registration.rs:281-289` -> `verify_native_host` at **403-483** -> `observe` at **437** -> `observe_native_process` at **1543-1548** -> producer.
- Paneless external registration: `registration.rs:139-152` -> `register_paneless_with_observer` at **169-279**, with an observation at **223** for each walked ancestor.
- Both ancestry walks refuse beyond 32 identities (`217-225`, `431-439`). Cost is one ps per observed ancestry step, not necessarily one ps per registration. The +60 does not mean 60 seats or 60 registrations.

**Scaling:** registration attempts x ancestry depth (at most 32 steps per attempt). No unconditional daemon timer or healthy-seat scan calls this producer.

The timer-driven Claude/Codex binders use nonempty seat ids (`claude_bind.rs:100`, `codex_bind.rs:292`), so their ordinary registration does not take the empty-id paneless branch at `registration.rs:144-151`.

The checked-in native client at f327a7d registers at startup/reconnect (`.copilot/extensions/pij/store.mjs:1176-1203`, `1354-1357`). Its healthy receiver heartbeats do **not** re-register every tick: `keepReceiverAlive` initializes `reregister = false` at **1209**, calls `/v1/inbox/heartbeat` at **1229-1235**, and only registers when that flag is true at **1218-1221**. Loaded client builds were not audited, so this source inspection is not proof that every live seat used that client version.

**Conditional timed retry concern:** a failed/stale/unavailable lease sets `reregister = true` at `store.mjs:1275` and retries with backoff; receiver errors can also reset `registered` at **1404**, with timed retries at **1447-1448**. Backoff starts at 250ms and caps at 5s (`18-19`). Persistent failure therefore can create timed per-seat registration/ps work: that recovery path has the forbidden launch shape while the failure persists, though the healthy heartbeat does not. The aggregated +60 cannot tell us whether startup, explicit registration, recovery or a mixture was active in that minute.

The historical control already had this site: +12, versus current +60 in approximately 60 seconds. #122 did not add or modify the producer.

## 2. `date.utc-offset`

Producer: **`crates/harnesses/src/proc.rs:322-347`**, counter at **323**, absolute `/bin/date +%z` at **324-327**. One date process per helper invocation.

It is a shared counter with multiple triggers:

| Trigger | References at f327a7d | Scaling |
|---|---|---|
| FYI claim/read HTTP requests | `crates/daemon/src/http/fyi.rs:81,130` | One per request, including an empty claim |
| Copilot typed-prompt FYI claim | `.copilot/extensions/pij/extension.mjs:127-138`, `store.mjs:1926-1936` -> HTTP path above | Prompt event, not an idle clock tick |
| FYI flush or delivery carrying FYIs | `crates/daemon/src/delivery/mod.rs:825,910,2402-2403` | Per flush/carrier attempt; plain sends skip the carrier offset |
| Claude adopt | `crates/daemon/src/http/identity.rs:749-755` | Per Claude adopt operation |
| Claude revive/conversation guard | `crates/daemon/src/http/revive.rs:178-189` | Per guard invocation |
| Live-Claude/double-seat record lookup | `crates/daemon/src/http/double_seat.rs:125-141` | One offset per lookup, outside its record loop |
| Pending spawned-Claude binder | `crates/daemon/src/claude_bind.rs:70-95` | Conditional per binder tick per eligible pending seat |

**Yes: the last path has the forbidden per-tick-per-seat shape.** `start_resilient_binders` ticks at `crates/daemon/src/lib.rs:664-681`; it is started with the delivery interval at **1055**, whose default is five seconds (`crates/core/src/config.rs:201`). `ClaudeBinder::bind_once` loops pending seats at **73**, and `bind_seat` calls the offset helper at **90** once it has a matching session record and a live PID witness.

The cohort is specifically pending spawned Claude seats (`claude_bind.rs:156-162`: Claude, no proc binding, pane and spawn id present, not tombstoned). It is not every healthy/bound seat. A mismatched record or refused registration can leave a seat pending, repeating the date launch next tick. At the default interval this can cost about 0.2P date launches/s for P qualifying pending seats. No current/past pending-seat census or caller-specific trace was taken, so this identifies the source shape, not the origin of each of the observed seven calls.

### Not new in #122

- The helper was already in the initial public release, `03ab38e9ffd0ce563a4b113d01c57c0166206e48`, dated 2026-10-02.
- The counter was added in `02edbd9a2d283012a8782c81ddd8adcf38cf67ca`, dated 2026-10-28.
- Diffing #122's merge against its first parent changes neither producer nor the listed caller files. The binder wiring is pre-existing; #122's lib changes construct the directory service.
- The Nov 4 control already recorded **date +9**. Current **+7** is a within-window counter increment, not a new site or a higher date rate against that control.

The shared label does not retain which caller produced each of the seven launches. Exact retrospective caller allocation is unavailable; do not assert that all seven came from the binder.

Unchanged source is not proof of unchanged activity volume: role or workload changes could still trigger old paths more often. This audit establishes that #122 did not introduce the date site or its listed callers.

## 3. What the original 774/s supports

Original window: 09:12 on 06 Nov, machine rate **774.0/s**. Its direct-daemon attribution was **1.1/s observed lower bound**, approximately **0.14%** of the total rate. The longer bracketing daemon counter window recorded 69/60.004265875s = **1.149918/s**, consistent in scale, but not the same 20s interval.

That 0.14% is not an upper bound on all daemon descendants or work submitted through it. The original classifier checks the immediate parent only; cargo or other jobs launched through `pij bg` can be deeper descendants classified as other.

**I cannot justify a numeric cargo share of the original 774/s. Unknown is not zero, and it is not permission to assign the remainder to five coders.** The original probe classified only direct daemon, status line and other parents, not cargo ancestry. It reported **599.4/s gone before classification** and **140.7/s node-named processes**. Top command names overlap the parent buckets and must not be added to them.

Neither cargo nor rustc appeared in that top-12 list. That is not proof of no build activity: short-lived children were missed, and a sampled comm can still be inherited before exec. Rustc fan-out is a hypothesis, not an allocation supported by the old receipt. CPU-heavy compiler jobs can also be long-lived rather than a high process-start rate.

## 4. Fresh ancestry diagnostic, not a reconstruction

A separate niced, load-guarded **20.000893625s** diagnostic began at **09:37:37.389904 +10:00**. PID 92221 remained the daemon. It read only BSD PID/parent/start/name metadata, not argv, and checked parent start times to reject reused-PID joins. Observer descendants were excluded. Cargo classification had a synthetic positive control and observed real cargo ancestry.

This sampler used 1ms pacing and ancestor walks, unlike the earlier unpaced sampler. Workload changed: initial snapshot **one cargo + two rustc**, final snapshot **neither**. An earlier 09:27 snapshot had two cargo + five rustc processes. These are process snapshots, not identification of five individual coder seats.

| Fresh observation | Count | Observed rate/s |
|---|---:|---:|
| PID-counter estimate, all new processes | 1959 | 97.9456 |
| Positive cargo ancestry | 55 | 2.7499 |
| Daemon-descendant non-cargo category | 18 | 0.9000 |
| Direct daemon children observed (overlapping dimension) | 10 | 0.5000 |
| Other resolved ancestry, including node-named | 504 | 25.1989 |
| Gone before metadata | 284 | 14.1994 |
| Not sampled, relative to PID-counter estimate | 1098 | 54.8975 |

The positive cargo observations are at least **2.81% of this fresh PID estimate**, not a cargo percentage for 09:12. The two unresolved groups total **1382/1959, 70.55%**, and remain unallocated. The daemon-descendant category can include agent-requested background jobs, not just core daemon logic. The direct-child count is an overlapping dimension, not an additional disjoint bucket.

The cargo-associated comm observations were 51 named cargo and four named rustc. They are **not counts of completed execs**: names can be inherited before exec. Likewise the raw diagnostic's `node-noncargo` label means no cargo witness at sample time, not proof that the process cannot later exec cargo.

Bracketing pings recorded **27 direct daemon launch-counter increments**, while the ancestry sampler saw only ten direct children. This independently demonstrates why sampled category counts are lower bounds. The recorder deltas were native registration ps +18, date +4, codex-process-files lsof +1, and four tmux operations +1 each (new window, attach, detach, owner option). Those pings include setup around the 20s PID window, so do not divide 27 by the window duration as an exact matched rate.

Maximum observed one-minute load was 34.78125, below the 40 stop rule. No compiler/build was started, paused or stopped by performance. No daemon lifecycle action, source change, dependency install, credential read or per-request trace was performed.

**Bottom line:** the original receipt puts the daemon's direct activity around one launch/s, not hundreds/s. It does not partition the other 773/s. The fresh sample positively identifies some cargo-family work but is a different, mostly unresolved workload; it cannot turn the earlier unknown bucket into measured rustc fan-out.
