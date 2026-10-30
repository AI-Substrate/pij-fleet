# The daemon's steady-state cost

Resource cost is a correctness property (AGENTS.md § Resource cost). This page says what the `pij-rs daemon` does on its own, with no request in flight, what that costs per seat, and how to measure it. Plan 185 set these numbers after the daemon was found at 200 % CPU and 8–11 GB RSS; its diagnosis is in [`docs/plans/185-daemon-cpu-memory/assets/evidence/diagnosis.md`](../plans/185-daemon-cpu-memory/assets/evidence/diagnosis.md).

## What runs on its own

| Loop | Interval | Per tick | Scales with |
|---|---|---|---|
| Pane observer | 600 ms | per tracked pane: control-client reads (`display-message`, `show-options`, `capture-pane`) and two `stat` calls on the pane's tap sink. The sink is opened only when it has grown. | tracked panes |
| Background jobs | 100 ms | one sqlite read of unfinished or unnotified jobs. A finished job's completion event is published once. A source whose owner is tombstoned waits for a resume or revive, as before: 2 in-process sqlite reads per tick (the owner's row, and the once-guard, which hits the first row it checks), no launches, and its refusal is logged once. | unfinished and waiting jobs |
| Sidecar bg worker | 100 ms | one `stat` of its state directory. It lists the directory only after it changes, and re-reads only running records. | running sidecar jobs |
| Delivery drain, binders | `delivery_interval_secs` | queue reads | queued deliveries |
| Death sweep | 5 s (`PIJ_RS_DEATH_SWEEP_MS`) | in-process liveness per live seat (no launch, plan 182) | live seats |
| PA watchdog | 60 s | registry and spine reads | PAs |

Launches per tick per seat: none. The launch sites and their rates are in [macos-defender.md](macos-defender.md).

## Request costs that used to scale with history

- **`GET /v1/events?since=…`** (OMP/pi seats on reconnect, chainglass every few seconds) used to decode the whole spine to learn its newest sequence: 6–7 s and 5 GB per attach on a 3 M-row spine. It now reads `MAX(seq)` and only the rows after the cursor, so an attach costs what the consumer is behind by, not the spine's size.
- **A stranded events source** (owner tombstoned) used to append its `bg.lost` completion every 100 ms for ever (2.4 M rows in three days) and log the same refusal ten times a second. The completion is now published once per job, and a repeating background-tick failure is logged once until it changes. The final turn still waits for the owner: a tombstone can be lifted by resume or revive.

## Tap files on disk

Each tracked pane has a tap sink, `<state-dir>/pane-signals/<pane>.raw`, appended to by `cat >>` through `tmux pipe-pane`. Two bounds hold:

- **Live taps.** At the first drain that finds nothing new after the daemon has consumed `TAP_TRIM_BYTES` (1 MiB) of a sink, the sink is truncated to zero and the reader's offset restarts at zero. A live sink therefore stays under 1 MiB plus one burst of output. Trimming only on a quiet drain matters: a writer can append between the length re-check and the truncation, and under a 4 MB/s burst that window lost about 1 KB in 2 of 35 runs. On a quiet drain the only write that can be lost is the first write of a new burst. A longer burst's later writes still show the pane active, but a burst of a single write (one keystroke echo, say) can vanish whole. The tap is a secondary signal, and the composer capture still sees the keystroke.
- **Orphans.** Once per boot, after retiring unwanted taps, the observer sweeps sinks whose pane its tmux server no longer lists. **Pane absence is the policy: it decides every removal.** The 5-minute mtime check (`ORPHAN_TAP_GRACE`) is a **brake only**: removing it would make the sweep remove the same set or more, never a different one. The observer logs one line naming what it removed (count and bytes), what it claimed, and what the brake spared.
- **Ownership.** A sink's name is only its pane id, so every adapter records each sink it creates in its own server's ledger, `<tap root>/.owners/<server>.list`, before the sink exists. The sweep considers only its own ledger, so daemons with distinct state dirs (each tap root is `<state-dir>/pane-signals`) never remove each other's sinks. Two servers sharing one tap root are not separated: the same pane id on both names the same file in both ledgers. Sinks written before ledgers existed are claimed once, by the default server's adapter only, and only into its ledger: the sweep still decides each removal. `.owners/legacy-claimed` records that the claim happened.

## Measure it

Take every reading twice, a known interval apart, on the same fleet before and after a change.

```sh
pid=$(pgrep -f 'pij-rs daemon$')
ps -o %cpu=,rss= -p "$pid"                              # CPU and RSS; repeat at +15 s, +60 s, +10 min
pij-rs --json ping | jq .data.spawns                    # launches by call site; diff two readings
just spawn-rate --seconds 20                            # machine-wide launches and the daemon's share
du -sh ~/.pij-rs/pane-signals                           # tap files
sqlite3 -readonly ~/.pij-rs/pij.sqlite 'select max(seq) from spine_events'   # spine growth; diff two readings
sample "$pid" 10 -file /tmp/daemon.sample               # where the CPU goes (read-only)
python3 docs/plans/185-daemon-cpu-memory/assets/evidence/sample-busy.py /tmp/daemon.sample 'pij_|THREAD'
```

`sample-busy.py` drops idle leaves (condition waits, `kevent`, blocking reads) and charges each remaining sample to every frame on its stack, so a code path's count divided by the per-thread sample count is the cores it uses.

`crates/daemon/tests/spine_attach_cost.rs` replays one cursor attach on a synthetic spine of any size (`PIJ_ATTACH_ROWS`); run it under `/usr/bin/time -l` for the peak RSS.

## Measured (plan 185)

Private daemon (temp state dir, private `-L` tmux server), same recorded workload on both builds: a 2,978,027-row spine like the live one, one events source whose owner is tombstoned, and a client that attaches with a cursor 10 events behind every 3.7 s, like chainglass. Release builds of main `d459dda` and this branch, 2026-10-29/30.

| | main | branch |
|---|---:|---:|
| CPU over 60 s | 37.2 s (62 % of a core) | 1.15 s (1.9 %) |
| RSS at +15 s / +60 s / +120 s | 4.74 / 5.26 / 5.63 GB, climbing | 17.5 / 17.6 / 17.6 MB, flat |
| Spine growth | 1,199 rows in 120 s (10/s), all `bg.lost` | 0 |
| `daemon.log` | 1,205 repeated "background reconciliation failed" lines in 120 s | 1 (logged once; the source waits for its owner) |
| Launches (`spawns.total`) | 2 | 2 |
| One cursor attach, 3 M-row spine | 6.3–7.2 s, 5.1 GB peak RSS | 0.4–1.2 ms (one 357 ms outlier), 14 MB peak |

The live machine reading before the fix (pid 95125, 2026-10-29): 197–208 % CPU, 8.4–9.1 GB RSS, 0.9 launches/s, 3.1 GB of tap files (3.19 GB of it orphans). The deployed after-reading is taken by the prime after merge.

Per seat, steady state: no launches; two `stat` calls and three control-client reads per tracked pane per 600 ms; at most `TAP_TRIM_BYTES` plus one burst of disk per pane; no spine growth unless something happens.
