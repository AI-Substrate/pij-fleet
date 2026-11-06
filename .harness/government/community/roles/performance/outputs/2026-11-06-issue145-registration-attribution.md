# Issue #145: residual registration cost

**Residual confirmed; exact per-seat rates are not observable with the existing counters/events.** The evidence identifies retired-session standby bridges as concrete suspects, not a measured few-second registration loop on healthy current holders.

## Measured windows

Both windows use daemon PID **88522** at both endpoints. The deployed merge is **2089a3cc0a838c5068444506bc4b55983521b9b2**. The second window used two authorized `pij-rs --json ping` calls under `nice -n 19 taskpolicy -b`; no registration, reconnect, roster, process probe or new seat was requested.

| Window, local +10 | Actual observation interval | Registration ps | Total launches | Registry `seat.put` events |
|---|---:|---:|---:|---:|
| 15:50:10-15:51:15, original #134 AFTER | 65.533985 s | 250 -> 292, +42, 0.640889/s | +46, 0.701926/s | 0 |
| 16:25:35.814-16:26:40.989, #145 follow-up | 65.175041 s | 1666 -> 1708, +42, 0.644418/s | 2458 -> 2506, +48, 0.736478/s | 0 |

The follow-up's other deltas were `ps.uds-peer-start` +4 (0.061373/s) and `git.pa-watchdog-common-dir` +2 (0.030687/s). All other sites were flat; date remained absent. These counters do not measure machine-wide launches, descendants or CPU.

The historical window contains 26 spine events; the fresh window contains 15. Neither contains a `seat.put`. These are bounded, read-only SQLite reads, not daemon registration requests. The fresh sequence range contains 5,000 rows and spans both sides of the observation window. The earlier bounded snapshot has 36 positive `seat.put` examples outside the measured window, including Copilot bootstrap writes around 15:45:14-15:45:18. Thus an empty result is not an absent/wrong event-kind filter.

This confirms persistence well beyond bootstrap. It does **not** give a registration-request count, an ancestry denominator or a per-seat frequency.

## Named evidence

Safe native-client diagnostics identify three retired addresses that entered standby and then lost their receiver lease. Registry metadata independently shows that each shares its recorded PID/start incarnation with a different current, untombstoned native address:

| Retired address | Current recorded owner of that host incarnation | Observed client diagnostic, local +10 |
|---|---|---|
| `pij-artificial-turtle` | `pij-successive-barracuda` | Nov 4 17:28:05 standby, `retryMs=30000`; 17:28:13 receiver lease unavailable, HTTP 400 |
| `pij-alert-lamprey` | `pij-worthwhile-sole` | Nov 6 08:14:06 standby, `retryMs=30000`; 08:14:10 receiver lease unavailable, HTTP 400 |
| `pij-breezy-panda` | `pij-worthwhile-sole` | Nov 6 08:14:10 standby, `retryMs=30000`; 08:14:25 receiver lease unavailable, HTTP 400 |

The complete diagnostic files are small (3,066 / 3,215 / 2,396 bytes). Their subsequent safe records show intermittent HTTP 409 reconnect episodes, most recently around 15:44:49-15:44:50, just before the bounce. They contain no later `standby-ended` or lease-restored record. This proves their recorded failure/standby histories, **not** their continued execution or exact attempts during either measurement window.

Two further retired aliases share a current recorded host: `pij-actual-junglefowl` with `pij-worthwhile-sole`, and `pij-fellow-leopard` with `pij-depressed-viper`. These are **metadata-only candidates**; no matching standby diagnostic was established for them. A retained descriptor is not a liveness measurement.

The three logged candidates and junglefowl report build `b24cb58f4f`; leopard reports `26fc3badbe`. The tracked Copilot `extension.mjs` and `store.mjs` have no diff from either reported revision to the reviewed deployed-source ancestor `5f4bafa`. The reported build is a startup Git identity, not an independently captured runtime-module hash, so this is source compatibility rather than proof of loaded bytes.

## Trigger paths and reuse

1. **Native extension lease recovery:** `store.mjs:1205-1284` normally sends heartbeats and sleeps for `renew_after_ms`, with no registration on a healthy answer. A stale/unavailable/malformed answer or transport error sets `reregister`; the next iteration calls `register()`. Ordinary error backoff starts at 250 ms and caps at 5 s. A `native-holder` refusal instead uses 30 s standby, interruptible by that session's own new prompt.
2. **Native receive recovery:** `store.mjs:1355-1449` registers initially and after retryable failures clear `registered`. An ordinary unanswered inbox deadline does not do this, and `receiver-gap` explicitly does not invalidate registration. Holder refusals enter the same 30 s standby. Session holds have a separate escalation to 60 s after ten minutes.
3. **Two potential retry callers per initialized bridge:** receive and heartbeat recovery can both call `register()`. The precedence object coordinates prompt freshness, but there is no shared registration single-flight in these paths. A standby context that had already started its heartbeat can therefore have both loops re-attesting. The three logged standby-plus-lease-failure histories warrant checking this first.
4. **Shim call preflight, not a timer:** `generation-router.ts:376-455` performs ambient paneless registration before eligible `inbox`, `send`, `whoami` or `phonehome` invocations when no tmux pane is present. It is not a universal preflight for every command. This is another possible source of unchanged registrations without a new descriptor event; no per-call attribution was collected.
5. **Not the general liveness fallback:** the measured key is recorded by `registration.rs:1752`'s native ancestry argv reader. Other process/liveness sites have different keys. This observation locates the work in registration validation, but cannot distinguish the callers above.

`register_native_with_observer()` verifies the host before `register_inner()` evaluates the current native holder (`registration.rs:306-349, 804-815`). Consequently a refused old-session attempt can already have paid the ancestry reads. Successful native-extension registrations proceed to a registry write, whereas unchanged paneless registrations can return before the write (`registration.rs:930-950`), **after** validation. `AncestryReads` caches only within one registration, not across repeated calls.

These are real repeating cost paths. They are not evidence that each healthy holder runs one every few seconds. Nor does an unchanged descriptor imply that its proof was reused.

## Why exact attribution stops here

The spawn counter is keyed only by launch site. `seat.put` records committed descriptor writes, not attempted/refused/no-op registrations (`crates/store/src/registry.rs:329-388`). Client `registered` is emitted only on the bridge's first successful registration (`store.mjs:1193-1203`), and the native reporter suppresses repeated standby diagnostics within an episode (`store.mjs:2053-2063`). Counting these lines cannot recover attempt counts.

Therefore **all 42 ancestry ps launches in each window remain unallocated by seat and trigger**. The named standby cohort is a source-and-log lead, not an exclusive explanation of that total. The evidence does not justify assigning the launches to the current owners, or treating the old addresses as confirmed live offenders.

## Coder handoff

First add bounded, in-process attribution at the existing registration/argv-counter boundary: requested/resolved address, registration mode, outcome (commit/unchanged/refused/error), and argv-read count. A client retry reason can distinguish bootstrap/lease/receive/standby/shim, but must be advisory, never authority. A capped diagnostic counter/ring avoids creating an unbounded spine event per retry. No new executable is needed.

Then correlate one matched window and target the proven caller. The concrete first lead is old-session standby plus independent heartbeat recovery, not a new s07/s11 daemon tick. Do not blanket-cache a positive attestation solely on PID/start/pane: process start survives exec, and C27's host/argv checks must remain fail-closed. Cheap negative rejection before costly proof, or fewer redundant client re-attestations, needs correctness/security review.

No product changes, local builds/tests, lifecycle actions or synthetic traffic were performed. Exact per-seat/per-trigger attribution remains **blocked on missing attempt telemetry**; the issue is not closed.

Public handoff: https://github.com/AI-Substrate/pij-fleet/issues/145#issuecomment-6678804771
