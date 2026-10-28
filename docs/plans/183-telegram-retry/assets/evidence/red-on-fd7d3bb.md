# Red on fd7d3bb (plan 183)

Scratch worktree at fd7d3bb plus ONLY the new tests (main-API variants: run_outbound->run_once, no timeouts field, new-API tests split out). Produced by the 183 coder seat, 2026-10-28T08:41:34Z.

## store: cargo test -p pij-store --test queue_retry_order
```
test retry_positive_delay_never_becomes_eligible_early ... FAILED
test telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off ... FAILED
test other_kinds_are_not_held_behind_a_backing_off_row ... ok

thread 'retry_positive_delay_never_becomes_eligible_early' (221850300) panicked at crates/store/tests/queue_retry_order.rs:75:9:
a 1ns retry must not be due before 1793176571.394470001s, stored 1793176571
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
thread 'telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off' (221850301) panicked at crates/store/tests/queue_retry_order.rs:139:5:
assertion `left == right` failed: a different chat proceeds
  left: JobId(2)
 right: JobId(4)

test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.24s

```

## sidecars: cargo test -p pij-sidecars --lib retry_tests  (delays printed in whole seconds: 0 = the 100 ms retry)
```
test telegram::retry_tests::a_429_on_a_text_send_waits_at_least_retry_after ... FAILED
test telegram::retry_tests::inbound_is_delivered_while_an_outbound_job_retries ... FAILED
test telegram::retry_tests::a_job_that_keeps_failing_backs_off_then_gives_up ... FAILED
test telegram::retry_tests::a_429_on_an_upload_waits_at_least_retry_after ... FAILED
test telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling ... FAILED
test telegram::retry_tests::a_failing_job_costs_a_bounded_number_of_calls_per_hour ... FAILED

thread 'telegram::retry_tests::a_429_on_a_text_send_waits_at_least_retry_after' (221870027) panicked at crates/sidecars/src/telegram_retry_tests.rs:248:5:
assertion `left == right` failed: retry_after honoured
  left: [0]
 right: [30]
thread 'telegram::retry_tests::inbound_is_delivered_while_an_outbound_job_retries' (221870032) panicked at crates/sidecars/src/telegram_retry_tests.rs:327:5:
assertion `left == right` failed
  left: []
 right: ["from the phone"]

thread 'telegram::retry_tests::a_job_that_keeps_failing_backs_off_then_gives_up' (221870030) panicked at crates/sidecars/src/telegram_retry_tests.rs:278:5:
assertion `left == right` failed
  left: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
 right: [2, 4, 8, 16, 32, 64, 128, 256, 300]

thread 'telegram::retry_tests::a_429_on_an_upload_waits_at_least_retry_after' (221870028) panicked at crates/sidecars/src/telegram_retry_tests.rs:260:5:
assertion `left == right` failed
  left: [0]
 right: [45]

thread 'telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling' (221870031) panicked at crates/sidecars/src/telegram_retry_tests.rs:368:6:
inbound delivered while the upload is still stalled: Elapsed(())

thread 'telegram::retry_tests::a_failing_job_costs_a_bounded_number_of_calls_per_hour' (221870029) panicked at crates/sidecars/src/telegram_retry_tests.rs:311:5:
assertion `left == right` failed: MAX_ATTEMPTS on the branch
  left: 14400
 right: 10

test result: FAILED. 0 passed; 6 failed; 0 ignored; 0 measured; 46 filtered out; finished in 1.98s

```

## daemon: cargo test -p pij-daemon --test shim_http -- producer reused symlinked
(producer = S2 coverage and the shim symlink test = N4 control: both green on main by design; X2 mutant is their red)
```
test a_symlinked_attachment_is_named_by_the_link_from_the_shim ... ok
test the_producer_queues_the_body_then_each_file_in_order ... ok
test a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy ... FAILED
thread 'a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy' (221892675) panicked at crates/daemon/tests/shim_http.rs:1275:5:
  left: 200
 right: 400
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 21 filtered out; finished in 0.19s
```

## cli: cargo test -p pij-cli --bin pij-rs symlinked
```
test tests::a_symlinked_attachment_keeps_the_typed_name ... FAILED

thread 'tests::a_symlinked_attachment_keeps_the_typed_name' (221900784) panicked at crates/cli/src/main.rs:3553:9:
assertion `left == right` failed: absolute as typed, symlink not resolved
  left: "/private/var/folders/mv/9mcvlzg504b158ctlswmgwph0000gn/T/pij-cli-attachment-link-49365-1793176693373025000-0/target.html"
 right: "/var/folders/mv/9mcvlzg504b158ctlswmgwph0000gn/T/pij-cli-attachment-link-49365-1793176693373025000-0/link.html"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 51 filtered out; finished in 0.00s

```

## new surfaces: compile-red on main
```
== cargo test -p pij-store --test queue_enqueue_all
error: could not compile `pij-store` (test "queue_enqueue_all") due to 8 previous errors
error[E0425]: cannot find type `Enqueued` in module `pij_core::ports`
error[E0432]: unresolved import `pij_core::ports::BatchEnqueue`
error[E0599]: no method named `enqueue_all` found for reference `&dyn pij_core::ports::Queue` in the current scope
== cargo test -p pij-daemon --test telegram_order
error: could not compile `pij-daemon` (test "telegram_order") due to 2 previous errors; 1 warning emitted
error[E0432]: unresolved import `pij_sidecars::telegram::MAX_ATTEMPTS`
error[E0599]: no method named `enqueue_all` found for struct `Arc<SqliteQueue>` in the current scope
== cargo test -p pij-sidecars --lib sweep
error: could not compile `pij-sidecars` (lib test) due to 2 previous errors
error[E0425]: cannot find function `sweep_staged` in module `super`
```
