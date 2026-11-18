# Retro: pij-yielding-newt, plan 229 (workstream 004 s03, cache proxy gate)

**Role:** coder/229, OMP on github-copilot/claude-opus-5.5-1m. **PM:** pm-004 (pij-outstanding-silkworm).

**Delivered:** the daemon side of the Copilot cache proxy: the spawn gate and provider env (`crates/daemon/src/cache_proxy.rs`, `build_spawn_plan`), the seat's recorded route (migration 0047), the receipt and directory, the 30 s watch with one notice per state change to each live prime, `/health.cache_proxy`, and the doctor rows. Built against the status-route contract with an own struct and a stub; three contract changes arrived mid-lane (req-13 model mapping, the first-read rule, `COPILOT_PROVIDER_VISION`) and each cost one small edit because the gate's decision is a pure function over one reading.

**magicWand:** a seat whose packet names a worktree should be unable to edit the main checkout by a relative path. Four strikes now (D-167-1, D-36-1, D-171-1, D-229-1); every one was caught by luck, not a guard.

**difficulties:**
- D-229-1: relative-path edits landed in the main checkout again (fourth strike).
- D-229-2: two lanes share `target-ws004`, so the shared `debug/pij-rs` was the other lane's binary halfway through the live proof; copy the binary out right after building it.
- A private smoke needs its own tmux server (`PIJ_TMUX_SERVER`) and that server's global env pointed at the private daemon (`PIJ_RS_ADDR`, `PIJ_RS_STATE_DIR`, `PIJ_SESSION_ID` unset), or spawned seats register with the production daemon. D-226-8's `just private-daemon` wish would carry this too.
