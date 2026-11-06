# Plan 213: shim latency, before and after (AC4, F44)

All runs on the fleet machine, 2026-11-06, with `nice -n 19` only (the prime's rule for timing real child processes). Seconds per call.

- **Before** is the installed shim (main's `pij-cli.cjs`, which starts node, then node again through tsx).
- **After** is this branch's `pij-cli.cjs` (one node start, then `pij-rs`).

## Interleaved, same load (the comparison to cite)

Eight rounds, each running every command once in turn, so all share the same load.

```
load (36.60400390625, 33.76708984375, 36.48779296875)
old pij claim --help     median 0.546s  min 0.339s  max 0.567s
new pij claim --help     median 0.085s  min 0.052s  max 0.198s
node -e 0                median 0.059s  min 0.041s  max 0.093s
pij-rs claim --help      median 0.021s  min 0.012s  max 0.036s
old pij claims           median 0.639s  min 0.371s  max 1.792s
new pij claims           median 0.115s  min 0.075s  max 0.454s
pij-rs claims            median 0.040s  min 0.024s  max 0.255s
load (37.5546875, 34.06884765625, 36.5634765625)
```

The forwarded call costs about one bare node start (0.059 s median) plus the native call. That is about 6.4x faster for `claim --help` and 5.6x for `claims`.

## Separate loops (noisy, kept for the record)

The first table ran before the change, the second after; they ran a few minutes apart, at different load.

```
18:52  up 11 days,  6:30, 3 users, load averages: 33.69 34.59 38.17
pij claims: 1.084 0.872 0.802 0.641 1.081
pij-rs claims: 0.374 0.225 0.242 0.08 0.071
pij directory templates: 0.653 0.609 0.396 0.608 0.697
pij-rs directory templates: 0.072 0.074 0.284 0.078 0.059
pij claim --help: 0.664 0.979 0.577 0.476 0.414
pij-rs claim --help: 0.199 0.196 0.032 0.183 0.206
18:52  up 11 days,  6:30, 3 users, load averages: 33.18 34.43 38.05

18:56  up 11 days,  6:34, 3 users, load averages: 36.16 33.42 36.49
pij(213) claims: 0.997 1.194 1.109 1.565 2.006
pij-rs claims: 0.124 0.114 0.198 0.076 0.134
pij(213) directory templates: 0.389 1.067 0.457 1.004 0.687
pij-rs directory templates: 0.076 0.225 0.07 0.064 0.231
pij(213) claim --help: 0.533 0.561 0.494 0.397 0.416
pij-rs claim --help: 0.066 0.057 0.045 0.083 0.053
18:57  up 11 days,  6:34, 3 users, load averages: 34.19 33.17 36.33
```
