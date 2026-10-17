# Plan 176: live seats whose id uses a now-excluded word

Read-only snapshot of the production daemon: `pij list --json` on 2026-10-17, re-taken after fix round 1 (53 seats, `unavailable: []`). Nothing was renamed, closed or messaged; Jordan decides. Existing ids keep working because ids are stored, not recomputed (AC3, see the execution log).

**41 of 53** listed seats match. A word matches when it is one hyphen-separated part of the id (or the whole name, for a ship).

| seat | harness | state | role | parent | excluded words |
|---|---|---|---|---|---|
| pij-additional-leeroy | copilot | idle | worker | pij-far-jackal | leeroy |
| pij-cheerful-orlandine | pi | idle | — | — | orlandine |
| pij-controversial-guanaco | claude | idle | worker | pij-far-jackal | controversial |
| pij-convinced-silverpine | claude | idle | — | — | silverpine |
| pij-critical-redridge | claude | idle | worker | pij-spatial-skink | critical, redridge |
| pij-crooked-thunderbluff | claude | idle | worker | pij-far-jackal | crooked, thunderbluff |
| pij-decisive-bolvar | claude | working | worker | pij-different-smelt | bolvar |
| pij-depressed-viper | copilot | idle | — | — | depressed, viper |
| pij-double-buru | claude | idle | pa | pij-fancy-thorn | buru |
| pij-extensive-riss | claude | idle | worker | pij-spatial-skink | riss |
| pij-fancy-thorn | claude | idle | prime | — | thorn |
| pij-fixed-elk | claude | idle | pa | pij-different-smelt | fixed |
| pij-fragile-meerkat | claude | idle | — | — | fragile |
| pij-free-flute | pi | idle | — | pij-dominant-vicuna | flute |
| pij-fundamental-ziller | claude | idle | worker | pij-fancy-thorn | ziller |
| pij-gay-thorn | claude | idle | — | — | gay, thorn |
| pij-genuine-gothik | claude | idle | — | — | gothik |
| pij-grotesque-zulfarrak | copilot | idle | — | — | grotesque, zulfarrak |
| pij-international-thorn | claude | idle | worker | pij-different-smelt | thorn |
| pij-irregular-apocalypse | pi | idle | — | pij-dominant-vicuna | apocalypse, irregular-apocalypse |
| pij-lonely-antelope | claude | idle | prime | — | lonely |
| pij-objective-diziet | copilot | idle | — | — | diziet |
| pij-passing-hoverfly | claude | idle | — | — | passing |
| pij-petite-guineafowl | claude | idle | worker | pij-sudden-gore | petite, guineafowl |
| pij-preferred-yura | claude | idle | worker | pij-fancy-thorn | yura |
| pij-prime-mandrill | copilot | idle | worker | pij-depressed-viper | prime, mandrill |
| pij-rational-emhyr | claude | idle | — | — | emhyr |
| pij-reluctant-azuregos | claude | idle | worker | pij-spatial-skink | azuregos |
| pij-resonant-stratholme | claude | idle | worker | pij-fancy-thorn | stratholme |
| pij-rival-riss | claude | idle | — | — | riss |
| pij-rubber-pigeon | claude | idle | — | pij-future-cicada | rubber |
| pij-scientific-dijkstra | claude | idle | — | — | dijkstra |
| pij-sleepy-dijkstra | claude | idle | worker | pij-different-smelt | sleepy, dijkstra |
| pij-sudden-gore | claude | idle | pm | pij-fancy-thorn | gore |
| pij-suspicious-meadowlark | omp | idle | prime | — | suspicious |
| pij-thoughtless-spoonbill | claude | idle | — | — | thoughtless |
| pij-unfortunate-blackbird | claude | working | worker | pij-weary-azshara | unfortunate, blackbird |
| pij-upper-wetlands | claude | idle | worker | pij-different-smelt | upper, wetlands |
| pij-weary-azshara | claude | idle | — | — | weary, azshara |
| pij-wily-planarian | claude | idle | — | — | wily |
| pij-xeric-redridge | claude | idle | — | — | redridge |

Parents named above but absent from `pij list` (shown for completeness): pij-dominant-vicuna (uses dominant).

pij-gay-thorn (the reported case) is in the list; it was doing research for Jordan and was left untouched.
