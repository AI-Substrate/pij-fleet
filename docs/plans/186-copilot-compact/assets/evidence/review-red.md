# PR #95 review fixes: red on c0258a2

New tests in `crates/daemon/tests/copilot_compact.rs` (S1, S3, N1), run on head `c0258a2` before any fix with `cargo test -p pij-daemon --test copilot_compact`:

```
test copilot_compact_is_never_typed_over_a_composer_draft ... ok
test copilot_compact_is_typed_through_the_pane_gate_and_bodies_stay_native ... ok
test copilot_compact_waits_out_an_idle_blip_inside_a_turn ... FAILED
test copilot_compact_waits_for_a_working_seat_and_runs_once_it_is_idle ... ok
test an_abandoned_copilot_control_claim_never_wedges_native_bodies ... FAILED
thread 'copilot_compact_waits_out_an_idle_blip_inside_a_turn' (250299554) panicked at crates/daemon/tests/copilot_compact.rs:330:5:
typed during an idle blip
thread 'an_abandoned_copilot_control_claim_never_wedges_native_bodies' (250299550) panicked at crates/daemon/tests/copilot_compact.rs:308:5:
  left: None
  right: Some("body-186")   (same failure, first red run)
test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.66s
```

The N1 draft test (real Copilot 1.0.105 frame, `❯ hello world`, caret at the end) passed on `c0258a2`; mutant M4 still turns it red.
