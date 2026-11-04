# The launch storm nobody's tests saw

tags: cost, launches, defender, history
hook: green tests said nothing about 250–480 launches a second; measure cost on purpose.

For weeks in 2026 the pij daemon launched 250–480 processes a second: three tmux clients per pane every 600 ms, a `ps` per liveness check, a ~20-process status line, and bare-name launches that cost 18 kernel processes each on macOS. Defender inspected every launch and burned about two cores. Every test was green the whole time (pij-fleet#62, plan 180). Later the daemon also sat at about 2 cores and 8–12 GB from a replay that re-read the spine on every subscriber (plans 185 and 187).

What to check on every PR: name each loop and its rate; anything per tick per seat; any launch by bare name in a repeated path; a new launch site missing from `spawn_sites.rs`; a read that scans a table where an index would do. Measure live before and after; a green suite proves nothing about cost.
