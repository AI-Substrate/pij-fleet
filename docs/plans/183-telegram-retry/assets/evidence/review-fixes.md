# Review fixes (PR #83 review by pij-review-pr83, verdict: merge after fixes)

Red: each witness written FIRST and run against the 15c28e2 source (B1, S1, N3, N4); N2 against 15c28e2 plus only its test hook (the injection point), before the restart logic existed.

## B1: cargo test -p pij-daemon --test shim_http -- keeps_every_live_copy  (RED on 15c28e2)
```
test a_staging_refusal_keeps_every_live_copy ... FAILED
test a_conflict_on_a_later_file_keeps_every_live_copy ... FAILED

failures:
--
thread 'a_staging_refusal_keeps_every_live_copy' (222691239) panicked at crates/daemon/tests/shim_http.rs:1396:5:
live doc-b1r#1 lost its staged copy
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
--
thread 'a_conflict_on_a_later_file_keeps_every_live_copy' (222691238) panicked at crates/daemon/tests/shim_http.rs:1360:5:
live doc-b1#1 lost its staged copy

--
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.17s

error: test failed, to rerun pass `-p pij-daemon --test shim_http`
```

## S1, N3, N4: cargo test -p pij-sidecars --lib -- the_sweep_never shutdown_stops inbound_backs_off  (RED on 15c28e2)
```
test telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends ... FAILED
test telegram::retry_tests::shutdown_stops_inbound_while_outbound_is_still_uploading ... FAILED
test telegram::retry_tests::inbound_backs_off_while_get_updates_fails ... FAILED

failures:
--
thread 'telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends' (222705475) panicked at crates/sidecars/src/telegram_retry_tests.rs:552:5:
a pending job's copy is never swept, however old
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
--
thread 'telegram::retry_tests::shutdown_stops_inbound_while_outbound_is_still_uploading' (222705474) panicked at crates/sidecars/src/telegram_retry_tests.rs:576:5:
assertion `left == right` failed: inbound stopped polling without waiting for the upload
  left: 36
 right: 16

---- telegram::retry_tests::inbound_backs_off_while_get_updates_fails stdout ----
--
thread 'telegram::retry_tests::inbound_backs_off_while_get_updates_fails' (222705473) panicked at crates/sidecars/src/telegram_retry_tests.rs:599:5:
attempts at 0 s and 1 s (then 3 s), not every tick: 76

--
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 57 filtered out; finished in 1.52s

error: test failed, to rerun pass `-p pij-sidecars --lib`
```

## N2: cargo test -p pij-sidecars --lib -- a_panicking_inbound  (RED on 15c28e2 + test hook only)
```
test telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded ... FAILED

failures:
--
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222801016) panicked at crates/sidecars/src/telegram.rs:445:21:
injected inbound panic
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
--
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222801016) panicked at crates/sidecars/src/telegram_retry_tests.rs:641:6:
inbound came back after two panicking passes: Elapsed(())

--
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 60 filtered out; finished in 8.07s

error: test failed, to rerun pass `-p pij-sidecars --lib`
```

## N1: refactor, no behaviour change (constant == old literal), so no red on 15c28e2. Pinned by telegram_order.rs the_producer_stages_where_the_sweep_looks; mutant N1 below.

## Mutants (each restored)
```
== MUTANT B1a release seam ignores live copies: exit 101
test a_staging_refusal_keeps_every_live_copy ... FAILED
test a_conflict_on_a_later_file_keeps_every_live_copy ... FAILED
thread 'a_staging_refusal_keeps_every_live_copy' (222908221) panicked at crates/daemon/tests/shim_http.rs:1406:5:
thread 'a_conflict_on_a_later_file_keeps_every_live_copy' (222908220) panicked at crates/daemon/tests/shim_http.rs:1364:5:
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.29s
error: test failed, to rerun pass `-p pij-daemon --test shim_http`
== MUTANT B1b Conflict path skips the live read: exit 101
test a_staging_refusal_keeps_every_live_copy ... ok
test a_conflict_on_a_later_file_keeps_every_live_copy ... FAILED
thread 'a_conflict_on_a_later_file_keeps_every_live_copy' (222912301) panicked at crates/daemon/tests/shim_http.rs:1364:5:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.16s
error: test failed, to rerun pass `-p pij-daemon --test shim_http`
== MUTANT B1c staging refusal skips the live read: exit 101
test a_conflict_on_a_later_file_keeps_every_live_copy ... ok
test a_staging_refusal_keeps_every_live_copy ... FAILED
thread 'a_staging_refusal_keeps_every_live_copy' (222917523) panicked at crates/daemon/tests/shim_http.rs:1406:5:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 24 filtered out; finished in 0.15s
error: test failed, to rerun pass `-p pij-daemon --test shim_http`
== MUTANT S1a sweep brake removed: exit 101
test telegram::retry_tests::the_shipped_loop_sweeps_expired_staged_copies ... ok
test telegram::document::tests::the_sweep_keeps_any_copy_a_live_job_sends ... FAILED
test telegram::document::tests::the_sweep_removes_copies_older_than_the_retention_only ... ok
test telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends ... FAILED
thread 'telegram::document::tests::the_sweep_keeps_any_copy_a_live_job_sends' (222920506) panicked at crates/sidecars/src/telegram_document_tests.rs:1134:5:
  left: 2
 right: 1
thread 'telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends' (222920509) panicked at crates/sidecars/src/telegram_retry_tests.rs:561:5:
test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.21s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT S1b worker sweeps with no live read: exit 101
test telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends ... FAILED
thread 'telegram::retry_tests::the_sweep_never_removes_a_copy_a_live_job_sends' (222922823) panicked at crates/sidecars/src/telegram_retry_tests.rs:561:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out; finished in 0.18s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT N2a pass not isolated (panic kills the loop): exit 101
test telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded ... FAILED
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222926892) panicked at crates/sidecars/src/telegram.rs:477:25:
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222926892) panicked at crates/sidecars/src/telegram_retry_tests.rs:641:6:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out; finished in 8.02s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT N2b restart not recorded: exit 101
test telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded ... FAILED
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222932471) panicked at crates/sidecars/src/telegram.rs:477:25:
pij-rs telegram inbound: pass panicked (injected inbound panic); restart 1 in 1000 ms
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222932471) panicked at crates/sidecars/src/telegram.rs:477:25:
pij-rs telegram inbound: pass panicked (injected inbound panic); restart 2 in 2000 ms
thread 'telegram::retry_tests::a_panicking_inbound_pass_is_restarted_and_recorded' (222932471) panicked at crates/sidecars/src/telegram_retry_tests.rs:645:5:
  left: 0
 right: 2
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out; finished in 3.05s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT N3 join outbound before signalling inbound: exit 101
test telegram::retry_tests::shutdown_stops_inbound_while_outbound_is_still_uploading ... FAILED
thread 'telegram::retry_tests::shutdown_stops_inbound_while_outbound_is_still_uploading' (222937836) panicked at crates/sidecars/src/telegram_retry_tests.rs:588:5:
  left: 37
 right: 17
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out; finished in 0.78s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT N4 no inbound backoff: exit 101
test telegram::retry_tests::inbound_backs_off_while_get_updates_fails ... FAILED
thread 'telegram::retry_tests::inbound_backs_off_while_get_updates_fails' (222944979) panicked at crates/sidecars/src/telegram_retry_tests.rs:611:5:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out; finished in 1.61s
error: test failed, to rerun pass `-p pij-sidecars --lib`
== MUTANT N1 the daemon spells the dir itself (drifted): exit 101
test the_producer_stages_where_the_sweep_looks ... FAILED
thread 'the_producer_stages_where_the_sweep_looks' (222974352) panicked at crates/daemon/tests/telegram_order.rs:172:5:
  left: "/var/folders/mv/9mcvlzg504b158ctlswmgwph0000gn/T/telegram-attachments-seam-70959-1793179104480626000-1/sidecars/telegram-attachment"
 right: "/var/folders/mv/9mcvlzg504b158ctlswmgwph0000gn/T/telegram-attachments-seam-70959-1793179104480626000-1/sidecars/telegram-attachments"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.06s
error: test failed, to rerun pass `-p pij-daemon --test telegram_order`
```
