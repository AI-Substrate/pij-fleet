# Jordan's standing preferences for the prime (pij-fleet)

Written by the current prime. The newest entry is last.

- 2026-11-10 (Jordan to pij-outstanding-silkworm): give status as a simple table when asked, and **always use emojis** as the visual overview (state at a glance: ✅ done/live, 🔄 in progress, ⏸️ held, ⏳ waiting, ❓ needs Jordan, ⚠️ risk).

## Rulings

- **2026-11-11, Jordan ("yes" to far-jackal's open question 3, asked again by pij-outstanding-silkworm): pij builds drop `taskpolicy -b`.** Every pij cargo build and test runs under `nice -n 19` only: one build at a time (shared-build's lock), 4 jobs (`.cargo/config.toml`). Background QoS confines cargo to the efficiency cores and throttles IO, so at fleet load (55-150) builds starved: salmon's clippy got 3% CPU over 24 min (retro plan 226), D-226-3 and D-226-5 took 24-35 min, and today's `pij-rs` rebuild after the target deletion sat for 20+ min while every seat's CLI was down. It supersedes the 06 Nov fleet cost rule (plan 208 ac-0010) and plan 209's `--no-run` QoS split (F63), which falls away since tests already ran nice-only.
