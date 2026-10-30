# Plan 186: red on main

The branch tests, copied onto an export of main `d459dda` (`git archive d459dda`), run with `cargo test -p pij-store --test queue_copilot_control` and `cargo test -p pij-daemon --test control_commands --test copilot_compact`. Each fails at the behaviour it defends: main refuses the control kind in the queue and refuses Copilot compact at the daemon.

## Store: `crates/store/tests/queue_copilot_control.rs`

```
test copilot_control_rows_are_delivery_rows_for_bookkeeping_on_the_fake ... FAILED
test copilot_compact_behind_bodies_never_runs_mid_claim_on_the_fake ... FAILED
test copilot_control_and_body_kinds_never_overlap_on_the_fake ... FAILED
test copilot_waiting_compact_never_blocks_native_parking_on_the_fake ... FAILED
test copilot_waiting_compact_never_blocks_native_parking_on_sqlite ... FAILED
test copilot_control_rows_are_delivery_rows_for_bookkeeping_on_sqlite ... FAILED
test copilot_control_and_body_kinds_never_overlap_on_sqlite ... FAILED
test copilot_compact_behind_bodies_never_runs_mid_claim_on_sqlite ... FAILED
test result: FAILED. 0 passed; 8 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.24s
thread 'copilot_control_rows_are_delivery_rows_for_bookkeeping_on_the_fake' (249585168) panicked at crates/store/tests/queue_copilot_control.rs:56:43:
enqueue delivery: Adapter { adapter: "testkit/fake-queue", message: "delivery operation requires kind delivery:pij-copilot, got delivery-control:pij-copilot" }
thread 'copilot_compact_behind_bodies_never_runs_mid_claim_on_the_fake' (249585164) panicked at crates/store/tests/queue_copilot_control.rs:56:43:
enqueue delivery: Adapter { adapter: "testkit/fake-queue", message: "delivery operation requires kind delivery:pij-copilot, got delivery-control:pij-copilot" }
```

## Daemon admission: `crates/daemon/tests/control_commands.rs`

```
test control_copilot_compact_queues_on_the_control_kind_not_the_native_body_kind ... FAILED
test control_shim_refuses_every_nonempty_body_channel_and_spoofed_owner ... ok
test control_native_refuses_unsupported_targets_and_invalid_shapes_without_enqueueing ... FAILED
test control_native_send_preserves_command_and_derived_sender ... ok
test control_shim_send_and_compact_self_use_the_command_path ... ok
test control_destructive_ownership_requires_pane_proof_not_asserted_actor ... ok
test control_native_strips_only_standard_empty_envelopes ... ok
test control_recorded_parent_can_send_destructive_commands ... ok
test result: FAILED. 6 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.77s
thread 'control_copilot_compact_queues_on_the_control_kind_not_the_native_body_kind' (249593099) panicked at crates/daemon/tests/control_commands.rs:173:5:
assertion `left == right` failed: Envelope { ok: false, command: "pij send", v: 2, data: None, meta: Some("E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls"), error: Some(Refused), details: None, raw_json: None }
  left: 400
 right: 200
thread 'control_native_refuses_unsupported_targets_and_invalid_shapes_without_enqueueing' (249593101) panicked at crates/daemon/tests/control_commands.rs:152:9:
expected E-RS-CONTROL-PANELESS


```

## Drain: `crates/daemon/tests/copilot_compact.rs`

```
test copilot_compact_is_never_typed_over_a_composer_draft ... FAILED
test copilot_compact_waits_for_a_working_seat_and_runs_once_it_is_idle ... FAILED
test copilot_compact_is_typed_through_the_pane_gate_and_bodies_stay_native ... FAILED
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
thread 'copilot_compact_is_never_typed_over_a_composer_draft' (249597135) panicked at crates/daemon/tests/copilot_compact.rs:155:5:
assertion `left == right` failed: {"command":"pij send","error":"refused","meta":"E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls","ok":false,"v":2}
```
