# Mutants (plan 183)

Each mutant applied to the branch source, the named test run, source restored (scripted: scratch mutant.py asserts a unique match and restores in finally).

```
== MUTANT M1 retry_after ignored: exit 101
test telegram::retry_tests::a_429_on_a_text_send_waits_at_least_retry_after ... FAILED
test telegram::retry_tests::a_429_on_an_upload_waits_at_least_retry_after ... FAILED
thread 'telegram::retry_tests::a_429_on_a_text_send_waits_at_least_retry_after' (221915002) panicked at crates/sidecars/src/telegram_retry_tests.rs:282:5:
  left: [2]
 right: [30]
thread 'telegram::retry_tests::a_429_on_an_upload_waits_at_least_retry_after' (221915003) panicked at crates/sidecars/src/telegram_retry_tests.rs:294:5:
  left: [2]
 right: [45]
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 55 filtered out; finished in 0.13s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT M2 inbound skipped on Retry (run_once): exit 101
test telegram::retry_tests::inbound_is_delivered_while_an_outbound_job_retries ... FAILED
thread 'telegram::retry_tests::inbound_is_delivered_while_an_outbound_job_retries' (221916095) panicked at crates/sidecars/src/telegram_retry_tests.rs:377:5:
  left: []
 right: ["from the phone"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.01s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT M3 no upload timeout: exit 101
test telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling ... FAILED
thread 'telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling' (221916866) panicked at crates/sidecars/src/telegram_retry_tests.rs:431:6:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 56 filtered out; finished in 5.16s
error: test failed, to rerun pass `-p pij-sidecars --lib`
Traceback (most recent call last):
  File [35m"/private/tmp/claude-501/-Users-jordanknight-pi-hacking-fleet-183-tg-retry/803eec18-96bf-4031-b6ca-518cec66dc56/scratchpad/mutant.py"[0m, line [35m4[0m, in [35m<module>[0m
    assert [1;31ms.count(old)==1[0m, (label, s.count(old))
           [1;31m^^^^^^^^^^^^^^^[0m
[1;35mAssertionError[0m: [35m('X2 producer drops the body', 0)[0m
== MUTANT M5 FIFO rule removed: exit 101
test telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off ... FAILED
test the_fifo_claim_uses_an_index_at_a_realistic_queue_size ... FAILED
thread 'telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off' (221924613) panicked at crates/store/tests/queue_retry_order.rs:139:5:
  left: JobId(2)
 right: JobId(4)
thread 'the_fifo_claim_uses_an_index_at_a_realistic_queue_size' (221924614) panicked at crates/store/tests/queue_retry_order.rs:252:10:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 2 filtered out; finished in 1.78s
error: test failed, to rerun pass `-p pij-store --test queue_retry_order`
== MUTANT M6 retry deadline truncated: exit 101
test retry_positive_delay_never_becomes_eligible_early ... FAILED
thread 'retry_positive_delay_never_becomes_eligible_early' (221929341) panicked at crates/store/tests/queue_retry_order.rs:75:9:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.05s
error: test failed, to rerun pass `-p pij-store --test queue_retry_order`
== MUTANT M7 N1 conflict check removed: exit 101
test a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy ... FAILED
thread 'a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy' (221934672) panicked at crates/daemon/tests/shim_http.rs:1275:5:
  left: 200
 right: 400
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 23 filtered out; finished in 0.16s
error: test failed, to rerun pass `-p pij-daemon --test shim_http`
== MUTANT X2 producer drops the body: exit 101
test the_producer_queues_the_body_then_each_file_in_order ... FAILED
thread 'the_producer_queues_the_body_then_each_file_in_order' (221950646) panicked at crates/daemon/tests/shim_http.rs:1215:21:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 23 filtered out; finished in 0.28s
error: test failed, to rerun pass `-p pij-daemon --test shim_http`
== MUTANT M8 one shipped loop (inbound behind outbound): exit 101
test telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling ... FAILED
thread 'telegram::retry_tests::a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling' panicked at crates/sidecars/src/telegram_retry_tests.rs:418:6:
inbound delivered while the upload is still stalled: Elapsed(())
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 56 filtered out; finished in 1.05s
```

| mutant | packet name | killed by |
|---|---|---|
| M1 retry_after ignored (`Some(backoff)`) | retry_after ignored | a_429_on_a_text_send / a_429_on_an_upload |
| M2 run_once returns before inbound on outbound error | inbound skipped on Retry | inbound_is_delivered_while_an_outbound_job_retries |
| M3 `.timeout(self.timeouts.upload)` removed | no timeout | a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling |
| X2 producer drops the body (`.take(0)`) | body dropped (X2) | the_producer_queues_the_body_then_each_file_in_order |
| M5 FIFO clause neutralised | (extra) | telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off |
| M6 retry deadline truncated | (extra) | retry_positive_delay_never_becomes_eligible_early |
| M7 N1 conflict check removed | (extra) | a_reused_msg_id_with_other_content_is_refused_and_leaves_no_copy |
| M8 one shipped loop | (extra) | a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling |

