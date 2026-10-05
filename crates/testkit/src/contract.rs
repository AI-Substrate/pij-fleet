//! Shared contract suites — one seam, one suite.
//!
//! Tenet 2: freeze the contract WITH its proof. A port's meaning lives here, not
//! in each implementation's own tests, so the fake and the real adapter answer
//! the SAME questions. When the SQLite `Registry` lands at tk-d4c3 it runs
//! [`registry_contract`] unchanged; if it disagrees with the fake, exactly one of
//! them is wrong and the suite says which assertion caught it.
//!
//! Each suite is `async` and takes `&dyn Port`, so it composes with any runtime:
//! core's tests drive it with [`crate::block_on`], adapters with tokio.

use pij_core::model::{
    DeliveryOrigin, Event, Harness, Job, JobId, Outcome, Pane, SeatDescriptor, SeatId, Seq,
    SystemState,
};
use std::path::Path;
use std::time::Duration;

use pij_core::ports::{
    DeferOutcome, DeliveryAck, DeliveryEnqueue, Queue, Registry, ReleaseOutcome, SeatFilter, Spine,
    TmuxPort,
};

/// Every promise the [`Registry`] port makes, in one place.
///
/// # Panics
/// On the first broken promise, naming which one.
pub async fn registry_contract(registry: &dyn Registry) {
    let id = SeatId::from("pij-contract-seat");

    // 1. An absent ROW is distinct from every value inside a present row.
    assert_eq!(
        registry.get(&id).await.expect("get must not fail"),
        None,
        "a seat that was never written must read back as absent, not as a default row"
    );

    // 2. SQL NULL and the empty string remain distinct values on a present row.
    let null_descriptor = SeatDescriptor::new(id.clone(), Harness::Pi, "/tmp/contract");
    let first = registry
        .put(null_descriptor.clone())
        .await
        .expect("put must not fail");
    assert_eq!(
        registry.get(&id).await.expect("get must not fail"),
        Some(null_descriptor.clone()),
        "a descriptor containing nullable fields must round-trip in full"
    );

    let mut empty_descriptor = SeatDescriptor::new("pij-contract-empty", Harness::Pi, "/tmp/empty");
    empty_descriptor.pane = Some(String::new());
    let second = registry
        .put(empty_descriptor.clone())
        .await
        .expect("put must not fail");
    assert_eq!(
        registry
            .get(&empty_descriptor.id)
            .await
            .expect("get must not fail"),
        Some(empty_descriptor.clone()),
        "an empty string is a value, not SQL NULL or an absent row"
    );

    // 3. Sequence numbers advance across writes.
    let third = registry
        .put(null_descriptor.clone())
        .await
        .expect("put must not fail");
    assert!(
        first < second && second < third,
        "sequence numbers must advance: got {first:?}, {second:?}, {third:?}"
    );

    // 4. Filters narrow, while an absent filter field means "do not filter".
    let others = registry
        .list(SeatFilter {
            harness: Some(Harness::Claude),
            ..SeatFilter::default()
        })
        .await
        .expect("list must not fail");
    assert!(
        others.is_empty(),
        "a filter must exclude non-matching seats; got {others:?}"
    );
    let empty_folder = registry
        .list(SeatFilter {
            folder: Some(String::new()),
            ..SeatFilter::default()
        })
        .await
        .expect("list must not fail");
    assert!(
        empty_folder.is_empty(),
        "an empty filter value is an exact value, not an absent filter"
    );
    let all = registry
        .list(SeatFilter::default())
        .await
        .expect("list must not fail");
    assert_eq!(all.len(), 2, "the default filter matches every row");

    // 5. A tombstone keeps the row, its marker and its reason observable through
    // both read operations. A seat that vanishes takes its post-mortem with it.
    let tombstone_seq = registry
        .tombstone(&id, "contract test")
        .await
        .expect("tombstone must not fail");
    assert!(tombstone_seq > third, "a tombstone is an ordered write");
    let tombstoned = registry
        .get(&id)
        .await
        .expect("get must not fail")
        .expect("a tombstoned seat remains readable");
    assert!(
        tombstoned.tombstoned_at.is_some(),
        "the post-mortem must expose when the tombstone was recorded"
    );
    assert_eq!(
        tombstoned.tombstone_reason.as_deref(),
        Some("contract test"),
        "the post-mortem must expose why the seat was tombstoned"
    );
    let listed = registry
        .list(SeatFilter::default())
        .await
        .expect("list must not fail");
    assert_eq!(
        listed.iter().find(|seat| seat.id == id),
        Some(&tombstoned),
        "get and list must expose identical tombstone facts"
    );

    // 6. Re-registering a live descriptor clears its old tombstone; otherwise a
    // live seat remains dead according to the same row.
    registry
        .put(null_descriptor.clone())
        .await
        .expect("put must revive the row");
    assert_eq!(
        registry.get(&id).await.expect("get must not fail"),
        Some(null_descriptor),
        "putting a live descriptor must clear the prior tombstone"
    );

    // 7. Tombstoning a seat that was never there is an error, not pretend work.
    let missing = SeatId::from("pij-never-existed");
    assert!(
        registry.tombstone(&missing, "nope").await.is_err(),
        "tombstoning an absent seat must fail loudly"
    );

    // 8. A seat's own busy/idle observation changes only `state` (plan 158):
    //    every other field survives, an unchanged state writes nothing, and a
    //    tombstoned seat is never revived by a late observation.
    let busy = SeatId::from("pij-contract-busy");
    let mut descriptor = SeatDescriptor::new(busy.clone(), Harness::Omp, "/tmp/contract");
    descriptor.role = Some("coder".to_string());
    descriptor.pane = Some("%9".to_string());
    registry
        .put(descriptor.clone())
        .await
        .expect("put busy seat");
    assert!(
        registry
            .set_activity(&busy, pij_core::model::SystemState::Working, None)
            .await
            .expect("set_activity")
            .is_some(),
        "idle -> working is a change"
    );
    let mut expected = descriptor.clone();
    expected.state = pij_core::model::SystemState::Working;
    let stored = registry.get(&busy).await.expect("get").expect("present");
    assert_eq!(
        (stored.state, stored.role.clone(), stored.pane.clone()),
        (expected.state, expected.role.clone(), expected.pane.clone()),
        "only the mechanical state changes"
    );
    assert!(
        registry
            .set_activity(&busy, pij_core::model::SystemState::Working, None)
            .await
            .expect("set_activity")
            .is_none(),
        "an unchanged state writes nothing"
    );
    registry
        .tombstone(&busy, "closed")
        .await
        .expect("tombstone");
    assert!(
        registry
            .set_activity(&busy, pij_core::model::SystemState::Working, None)
            .await
            .expect("set_activity")
            .is_none(),
        "a tombstoned seat takes no activity"
    );
    let retired = registry.get(&busy).await.expect("get").expect("present");
    assert!(retired.tombstoned_at.is_some(), "never revived");
    assert_eq!(
        retired.state,
        pij_core::model::SystemState::Idle,
        "a tombstone ends the turn it interrupted"
    );
}

/// Every promise the [`Spine`] port makes, in one place.
///
/// # Panics
/// On the first broken promise, naming which one.
pub async fn spine_contract(spine: &dyn Spine) {
    let seat_a = SeatId::from("pij-spine-a");
    let seat_b = SeatId::from("pij-spine-b");
    assert!(
        spine
            .tail(None, Seq(0))
            .await
            .expect("initial tail")
            .is_empty(),
        "an empty spine tails as an empty list, not an error"
    );

    let event = |kind: &str, seat: Option<SeatId>, payload: &str| Event {
        seq: None,
        v: 1,
        at: 1_724_800_000_000,
        kind: kind.to_string(),
        seat,
        payload: payload.to_string(),
    };
    let first_event = event("report", Some(seat_a.clone()), "{\"n\":1}");
    let second_event = event("receipt", Some(seat_b.clone()), "");
    let third_event = event("future-kind", Some(seat_a.clone()), "opaque");
    let fourth_event = event("message", None, "fleet-wide");

    let first = spine
        .append(first_event.clone())
        .await
        .expect("first append");
    let second = spine
        .append(second_event.clone())
        .await
        .expect("second append");
    let third = spine
        .append(third_event.clone())
        .await
        .expect("third append");
    let fourth = spine
        .append(fourth_event.clone())
        .await
        .expect("fourth append");
    assert!(
        first < second && second < third && third < fourth,
        "append must allocate a strictly increasing total order"
    );

    let after_first = spine.tail(None, first).await.expect("tail after first");
    let mut expected_second = second_event;
    expected_second.seq = Some(second);
    let mut expected_third = third_event;
    expected_third.seq = Some(third);
    let mut expected_fourth = fourth_event;
    expected_fourth.seq = Some(fourth);
    assert_eq!(
        after_first,
        vec![
            expected_second.clone(),
            expected_third.clone(),
            expected_fourth.clone(),
        ],
        "tail is exclusive, oldest-first, assigns each durable cursor, and preserves opaque facts"
    );

    let just_a = spine
        .tail(Some(&seat_a), Seq(0))
        .await
        .expect("seat-filtered tail");
    let mut expected_first = first_event;
    expected_first.seq = Some(first);
    assert_eq!(
        just_a,
        vec![expected_first, expected_third.clone()],
        "a seat filter narrows without losing each returned event's durable cursor"
    );
    assert_eq!(
        spine
            .latest_matching(&seat_a, &["report", "future-kind"])
            .await
            .expect("latest matching"),
        Some(expected_third),
        "latest is mandatory-seat-scoped, kind-filtered, and returns one highest-seq fact"
    );
    assert!(
        spine
            .latest_matching(&seat_b, &["future-kind"])
            .await
            .expect("no matching kind")
            .is_none()
    );
    assert!(
        spine.latest_matching(&seat_a, &[]).await.is_err(),
        "empty kinds must refuse rather than broaden the bounded query"
    );
    assert_eq!(
        spine.tail(None, third).await.expect("tail after third"),
        vec![expected_fourth],
        "an event with no seat remains visible in an unfiltered tail"
    );
    assert!(
        spine
            .tail(None, fourth)
            .await
            .expect("tail newest")
            .is_empty(),
        "tail since the newest cursor is empty"
    );
    let older_message = event(
        "control-pointer",
        Some(seat_a.clone()),
        "{\"msg_id\":\"m-older\"}",
    );
    spine
        .append(older_message.clone())
        .await
        .expect("older per-message fact");
    spine
        .append(event(
            "control-pointer",
            Some(seat_a.clone()),
            "{\"msg_id\":\"m-newer\"}",
        ))
        .await
        .expect("newer per-message fact");
    let found = spine
        .latest_matching_message(&seat_a, "control-pointer", "m-older")
        .await
        .expect("bounded per-message lookup")
        .expect("older message fact remains addressable");
    assert_eq!(found.kind, "control-pointer");
    assert_eq!(found.payload, older_message.payload);
}

/// Every promise the [`Queue`] port makes (R4).
///
/// # Panics
/// On the first broken promise, naming which one.
pub async fn queue_contract(queue: &dyn Queue) {
    let kinds = vec!["deliver".to_string()];

    let job = |dedupe: &str, serial: &str| Job {
        kind: "deliver".to_string(),
        serial_key: serial.to_string(),
        payload: "{}".to_string(),
        dedupe_key: dedupe.to_string(),
        dedupe_origin: None,
        attempt: 0,
    };

    // 1. Dedupe: N rapid submits collapse to ONE live row, and every caller gets
    //    the id of that row rather than a fresh one it will wait on for ever.
    let first = queue.enqueue(job("d1", "seat-a")).await.expect("enqueue");
    let again = queue.enqueue(job("d1", "seat-a")).await.expect("enqueue");
    assert_eq!(
        first, again,
        "a duplicate submit must return the live row's id, not a new one"
    );

    // 2. Serialization: two jobs for the same entity are never claimed at once.
    queue.enqueue(job("d2", "seat-a")).await.expect("enqueue");
    let claimed = queue
        .claim(&kinds, "worker-1")
        .await
        .expect("claim")
        .expect("a job is available");
    let blocked = queue.claim(&kinds, "worker-2").await.expect("claim");
    assert!(
        blocked.is_none(),
        "a second job for a busy serial key must not be claimable: got {blocked:?}"
    );

    // 3. Acking frees the key, and the next job becomes claimable — otherwise
    //    one entity's queue wedges for ever after a single job.
    queue.ack(claimed.0, Outcome::Done).await.expect("ack");
    let next = queue.claim(&kinds, "worker-2").await.expect("claim");
    assert!(
        next.is_some(),
        "acking must free the serial key for the next job"
    );

    // 4. A kind nobody asked for is not claimed.
    let none: Option<(JobId, Job)> = queue
        .claim(&["never-enqueued".to_string()], "worker-3")
        .await
        .expect("claim");
    assert!(none.is_none(), "claim must respect the kind filter");

    // Peek is observation only: repeated calls return the same row, and the row
    // remains available to the first real claimer.
    let peek_kind = vec!["peek-only".to_string()];
    let peek_id = queue
        .enqueue(Job {
            kind: peek_kind[0].clone(),
            serial_key: "seat-peek".to_string(),
            payload: "peek-body".to_string(),
            dedupe_key: "peek-1".to_string(),
            dedupe_origin: None,
            attempt: 0,
        })
        .await
        .expect("enqueue peek row");
    let first_peek = queue
        .peek(&peek_kind)
        .await
        .expect("peek")
        .expect("peek row");
    let second_peek = queue
        .peek(&peek_kind)
        .await
        .expect("peek again")
        .expect("same peek row");
    assert_eq!(first_peek, second_peek);
    assert_eq!(first_peek.0, peek_id);
    assert_eq!(first_peek.1.payload, "peek-body");
    let claimed_after_peek = queue
        .claim(&peek_kind, "peek-witness")
        .await
        .expect("claim after peek")
        .expect("peek did not consume row");
    assert_eq!(claimed_after_peek.0, peek_id);
    assert_eq!(
        queue.peek(&peek_kind).await.expect("peek claimed row"),
        Some(claimed_after_peek),
        "a non-destructive observer still sees a live row during another reader's claim"
    );

    // 5. Destination delivery dedupe spans terminal acknowledgement and replays
    //    the evidence that was actually recorded, not a synthetic queue claim.
    let delivery = Job {
        kind: "delivery:seat-delivered".to_string(),
        serial_key: "seat-delivered".to_string(),
        payload: "{}".to_string(),
        dedupe_key: "delivered-1".to_string(),
        dedupe_origin: None,
        attempt: 0,
    };
    let first = queue
        .enqueue_delivery(delivery.clone())
        .await
        .expect("enqueue delivery");
    let DeliveryEnqueue::Queued {
        job_id: first,
        not_before_ms: first_not_before,
    } = first
    else {
        panic!("a new delivery must queue");
    };
    let collapsed = queue
        .enqueue_delivery(delivery.clone())
        .await
        .expect("collapse live delivery");
    let DeliveryEnqueue::Queued {
        job_id: collapsed,
        not_before_ms: collapsed_not_before,
    } = collapsed
    else {
        panic!("a live delivery must remain queued");
    };
    assert_eq!(collapsed, first, "live-row collapse keeps the same id");
    assert_eq!(
        collapsed_not_before, first_not_before,
        "live-row collapse keeps the persisted schedule"
    );
    let claimed = queue
        .claim(std::slice::from_ref(&delivery.kind), "delivery-worker")
        .await
        .expect("claim delivery")
        .expect("delivery row");
    assert_eq!(claimed.0, first);
    let acknowledged = queue
        .ack_delivery(first, DeliveryOrigin::ReaderRead)
        .await
        .expect("record delivered id");
    assert_eq!(
        acknowledged,
        DeliveryAck {
            recipient: SeatId::from("seat-delivered"),
            msg_id: "delivered-1".to_string(),
            origin: DeliveryOrigin::ReaderRead,
        },
        "ack authority returns identity from the claimed job"
    );
    assert_eq!(
        queue
            .enqueue_delivery(delivery.clone())
            .await
            .expect("consult delivered id"),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
    );
    // Plan 164 review F02: identity is (origin, msg_id), origin in its own
    // field. The same id from a paired machine is a new message, neither the
    // delivered local one nor collapsed into it; its exact retry collapses.
    let forwarded = Job {
        dedupe_origin: Some("laptop".to_string()),
        ..delivery
    };
    let DeliveryEnqueue::Queued {
        job_id: forwarded_id,
        ..
    } = queue
        .enqueue_delivery(forwarded.clone())
        .await
        .expect("a peer's same id")
    else {
        panic!("a peer's same id is a new message, not the delivered local one");
    };
    assert_ne!(forwarded_id, first);
    let DeliveryEnqueue::Queued {
        job_id: retried, ..
    } = queue
        .enqueue_delivery(forwarded)
        .await
        .expect("the peer's exact retry")
    else {
        panic!("the peer's retry stays queued");
    };
    assert_eq!(
        retried, forwarded_id,
        "an exact retry from that peer collapses"
    );

    // R4-AMEND-4, in the SHARED contract so fake and real cannot drift: the claim
    // is atomic, it reports the ORIGINAL observation rather than the new caller's
    // guess, and a released claim is claimable again.
    let claimant = SeatId::from("seat-claimed");
    assert_eq!(
        queue
            .note_delivered(&claimant, "claim-1", None, DeliveryOrigin::ReaderRead)
            .await
            .expect("first claim"),
        None
    );
    assert_eq!(
        queue
            .note_delivered(
                &claimant,
                "claim-1",
                None,
                DeliveryOrigin::InjectedToTransport
            )
            .await
            .expect("second claim"),
        Some(DeliveryOrigin::ReaderRead),
        "a claimed message reports what was ORIGINALLY observed"
    );
    // Plan 164 review F02: the ledger key is (recipient, sender machine,
    // msg_id). The same id from a paired machine is another message, with its
    // own claim; an exact repeat from that machine is the same one.
    assert_eq!(
        queue
            .note_delivered(
                &claimant,
                "claim-1",
                Some("laptop"),
                DeliveryOrigin::ReaderRead
            )
            .await
            .expect("a peer's claim"),
        None,
        "a peer's `claim-1` is not the local `claim-1`"
    );
    assert_eq!(
        queue
            .note_delivered(
                &claimant,
                "claim-1",
                Some("laptop"),
                DeliveryOrigin::TypedToPane
            )
            .await
            .expect("the peer's retry"),
        Some(DeliveryOrigin::ReaderRead)
    );
    queue
        .forget_delivered(&claimant, "claim-1", None)
        .await
        .expect("release the claim");
    assert_eq!(
        queue
            .note_delivered(
                &claimant,
                "claim-1",
                Some("laptop"),
                DeliveryOrigin::TypedToPane
            )
            .await
            .expect("the peer's claim survives the local release"),
        Some(DeliveryOrigin::ReaderRead)
    );
    assert_eq!(
        queue
            .note_delivered(&claimant, "claim-1", None, DeliveryOrigin::ReaderRead)
            .await
            .expect("re-claim"),
        None,
        "a released claim is claimable again: that delivery never happened"
    );

    // An UNCLAIMED ack is refused by both implementations (review round-1 F6,
    // which the contract never asserted).
    queue
        .ack(JobId(u64::MAX), Outcome::Done)
        .await
        .expect_err("acking a job nobody claimed must be refused");

    // PLAN 136's TWO VERBS, pinned HERE because the reviewer found them pinned
    // only by two independent hand-written suites — and drifting (f3). The pair
    // decides whether a message a human is typing over comes back at the right
    // moment, so a fake that is more permissive than the store makes every green
    // typing test a false negative.
    let typing = Job {
        kind: "delivery:seat-typing".to_string(),
        serial_key: "seat-typing".to_string(),
        payload: "{}".to_string(),
        dedupe_key: "typing-1".to_string(),
        dedupe_origin: None,
        attempt: 0,
    };
    let DeliveryEnqueue::Queued { .. } = queue
        .enqueue_delivery(typing.clone())
        .await
        .expect("enqueue the typing delivery")
    else {
        panic!("a new delivery must queue");
    };
    let typing_kinds = vec!["delivery:seat-typing".to_string()];
    let (claimed_typing, _) = queue
        .claim(&typing_kinds, "typing-worker")
        .await
        .expect("claim the typing delivery")
        .expect("a queued delivery is claimable");

    // DEFER moves a live row back to pending WITHOUT charging it an attempt: a
    // hold is not a failed delivery, and inflating `attempt` would corrupt the
    // backoff and budget logic that reads it.
    let deferred = queue
        .defer(claimed_typing, Duration::from_secs(60))
        .await
        .expect("defer a claimed delivery");
    assert!(
        matches!(deferred, DeferOutcome::Deferred { .. }),
        "deferring a live row reports the identity the QUEUE holds: {deferred:?}"
    );
    assert!(
        queue
            .claim(&typing_kinds, "typing-worker")
            .await
            .expect("claim during the deferral")
            .is_none(),
        "a deferred row is not claimable until its deadline: this is the whole hold"
    );

    // RELEASE clears the deferral and the row is immediately claimable again.
    let released = queue
        .release_deferred(claimed_typing)
        .await
        .expect("release the deferral");
    assert!(
        matches!(released, ReleaseOutcome::Released { .. }),
        "releasing a deferred row names it: {released:?}"
    );
    let (reclaimed, reclaimed_job) = queue
        .claim(&typing_kinds, "typing-worker")
        .await
        .expect("claim after release")
        .expect("a released row is eligible at once");
    assert_eq!(reclaimed, claimed_typing, "the SAME row comes back");
    assert_eq!(
        reclaimed_job.attempt, 0,
        "defer/release must not charge an attempt"
    );

    // RELEASE IS PENDING-ONLY. The row is RUNNING now, and a duplicate release —
    // which a restarted extension issues routinely — must NOT revoke the claim a
    // live reader is executing, or that message goes out twice.
    assert!(
        matches!(
            queue
                .release_deferred(reclaimed)
                .await
                .expect("release a running row"),
            ReleaseOutcome::NotDeferred
        ),
        "a running row is left alone, and says so"
    );
    assert!(
        queue
            .claim(&typing_kinds, "other-worker")
            .await
            .expect("claim after the no-op release")
            .is_none(),
        "the live reader still owns its row after a duplicate release"
    );

    // Absent and terminal are DIFFERENT facts: an absent job on release is a
    // caller bug, a terminal one is a benign race, and one silent no-op for both
    // leaves an operator unable to tell them apart.
    assert!(
        matches!(
            queue
                .defer(JobId(u64::MAX), Duration::from_secs(1))
                .await
                .expect("defer an absent job"),
            DeferOutcome::NotLive { .. }
        ),
        "deferring a job nobody has is a typed no-op, never an error"
    );
    queue
        .ack(reclaimed, Outcome::Done)
        .await
        .expect("ack the typing delivery");
    assert!(
        matches!(
            queue
                .release_deferred(reclaimed)
                .await
                .expect("release a terminal job"),
            ReleaseOutcome::NotLive { .. }
        ),
        "a terminal row reports not-live rather than resurrecting"
    );

    // Holding is claim ownership, never delivered-id evidence. The primitive
    // rejects stale attempts as well as pending, terminal and foreign claims.
    let holder = SeatId::from("pij-heartbeat");
    let held = Job {
        kind: pij_core::delivery::delivery_kind(&holder),
        serial_key: holder.to_string(),
        payload: serde_json::json!({"to":holder,"command":null}).to_string(),
        dedupe_key: "heartbeat-body".into(),
        dedupe_origin: None,
        attempt: 0,
    };
    let held_kinds = [held.kind.clone()];
    let held_id = queue
        .enqueue(held.clone())
        .await
        .expect("enqueue heartbeat body");
    assert!(
        !queue
            .heartbeat_delivery(held_id, &holder, 0)
            .await
            .expect("pending heartbeat")
    );
    assert_eq!(
        queue
            .terminal_delivery_state(held_id, &holder)
            .await
            .expect("pending state"),
        None
    );
    queue
        .claim(&held_kinds, holder.as_str())
        .await
        .expect("claim body")
        .expect("body");
    assert!(
        !queue
            .heartbeat_delivery(held_id, &SeatId::from("pij-wrong"), 0)
            .await
            .expect("foreign heartbeat")
    );
    assert!(
        !queue
            .heartbeat_delivery(held_id, &holder, 1)
            .await
            .expect("stale heartbeat")
    );
    assert!(
        queue
            .heartbeat_delivery(held_id, &holder, 0)
            .await
            .expect("body heartbeat")
    );
    assert_eq!(
        queue
            .enqueue(held.clone())
            .await
            .expect("dedupe the running body"),
        held_id,
        "running claims remain live for ordinary enqueue deduplication"
    );
    assert!(
        matches!(queue.enqueue_delivery(held.clone()).await.expect("same body remains undelivered"),
        DeliveryEnqueue::Queued { job_id, .. } if job_id == held_id)
    );
    queue
        .retry(held_id, Duration::ZERO)
        .await
        .expect("retry heartbeat body");
    queue
        .claim(&held_kinds, holder.as_str())
        .await
        .expect("reclaim body")
        .expect("body");
    assert!(
        !queue
            .heartbeat_delivery(held_id, &holder, 0)
            .await
            .expect("old attempt heartbeat")
    );
    assert!(
        queue
            .heartbeat_delivery(held_id, &holder, 1)
            .await
            .expect("new attempt heartbeat")
    );
    assert_eq!(
        queue
            .terminal_delivery_state(held_id, &holder)
            .await
            .expect("running state"),
        None
    );
    queue
        .ack_delivery(held_id, DeliveryOrigin::ReaderRead)
        .await
        .expect("consume body");
    assert!(
        !queue
            .heartbeat_delivery(held_id, &holder, 1)
            .await
            .expect("terminal heartbeat")
    );
    assert_eq!(
        queue
            .terminal_delivery_state(held_id, &holder)
            .await
            .expect("done state"),
        Some("done")
    );
    assert_eq!(
        queue
            .terminal_delivery_state(held_id, &SeatId::from("pij-wrong"))
            .await
            .expect("foreign state"),
        None
    );
    for (dedupe, payload) in [
        (
            "heartbeat-control",
            serde_json::json!({"to":holder,"command":"compact"}).to_string(),
        ),
        ("heartbeat-invalid", "not-json".into()),
    ] {
        let mut rejected = held.clone();
        rejected.dedupe_key = dedupe.into();
        rejected.payload = payload;
        let rejected_id = queue
            .enqueue(rejected)
            .await
            .expect("enqueue excluded claim");
        queue
            .claim(&held_kinds, holder.as_str())
            .await
            .expect("claim excluded row")
            .expect("row");
        assert!(
            !queue
                .heartbeat_delivery(rejected_id, &holder, 0)
                .await
                .expect("excluded heartbeat")
        );
        assert!(
            queue
                .claimed_delivery(rejected_id)
                .await
                .expect("claim remains")
                .is_some()
        );
        queue
            .ack(rejected_id, Outcome::Done)
            .await
            .expect("finish excluded claim");
        assert_eq!(
            queue
                .terminal_delivery_state(rejected_id, &holder)
                .await
                .expect("excluded terminal state"),
            None
        );
    }
}

/// A seat descriptor with the shape most contract tests want.
pub fn sample_seat(id: &str) -> SeatDescriptor {
    SeatDescriptor {
        state: SystemState::Idle,
        ..SeatDescriptor::new(id, Harness::Pi, "/tmp/pij-contract")
    }
}

/// Implementation-owned data required to run the destructive tmux contract.
///
/// The fixture, not the suite, creates and owns the isolated session. Real
/// fixtures must tear that session down from `Drop`, including unwind paths.
pub trait TmuxContractFixture {
    /// A pane owned exclusively by this contract run.
    fn pane(&self) -> &Pane;

    /// Exact initial capture prepared in the owned pane.
    fn initial_capture(&self) -> &str;

    /// Literal text the suite may type into the owned pane.
    fn sent_keys(&self) -> &str;

    /// Fragment that must be visible after [`TmuxPort::send_keys`].
    fn sent_keys_capture(&self) -> &str;

    /// Name for the disposable window the suite creates and kills.
    fn new_window_name(&self) -> &str;

    /// Caller-owned sink used for the pane-tap lifecycle.
    fn tap_sink(&self) -> &Path;
}

/// Every portable promise of [`TmuxPort`], over an implementation-owned fixture.
///
/// Adapter-specific safety properties (fresh-resolution kill brake, exact argv,
/// capture boundaries, and `pane_in_mode`) remain adapter tests because the
/// frozen fake intentionally records rather than implements those mechanisms.
///
/// # Panics
/// On the first broken promise, naming which operation disagreed.
pub async fn tmux_contract(tmux: &dyn TmuxPort, fixture: &dyn TmuxContractFixture) {
    let pane = fixture.pane();
    let listed = tmux.list_panes().await.expect("list_panes must not fail");
    assert!(
        listed.contains(pane),
        "the fixture pane must appear byte-identical in list_panes: {listed:?}"
    );

    assert_eq!(
        tmux.capture(&pane.id, 100)
            .await
            .expect("capture must not fail"),
        fixture.initial_capture(),
        "capture must return the fixture's visible text"
    );
    assert!(
        !tmux
            .user_typing(&pane.id)
            .await
            .expect("user_typing must not fail"),
        "the fixture starts outside a tmux interaction mode"
    );

    assert!(
        tmux.drain_pane_tap(&pane.id).await.is_err(),
        "an unattached tap is absent, not an empty observation"
    );
    assert_eq!(
        tmux.pane_tap_sink(&pane.id)
            .await
            .expect("tap ownership read before attach"),
        None
    );
    tmux.attach_pane_tap(&pane.id, fixture.tap_sink())
        .await
        .expect("attach_pane_tap must not fail");
    assert_eq!(
        tmux.pane_tap_sink(&pane.id)
            .await
            .expect("tap ownership read after attach"),
        Some(fixture.tap_sink().to_path_buf())
    );
    assert!(
        tmux.drain_pane_tap(&pane.id)
            .await
            .expect("an attached quiet tap must drain")
            .is_empty(),
        "a fresh quiet tap has no unread bytes"
    );
    tmux.detach_pane_tap(&pane.id)
        .await
        .expect("detach_pane_tap must not fail");
    assert_eq!(
        tmux.pane_tap_sink(&pane.id)
            .await
            .expect("tap ownership read after detach"),
        None
    );
    assert!(
        tmux.drain_pane_tap(&pane.id).await.is_err(),
        "a detached tap returns to absent"
    );

    tmux.send_keys(&pane.id, fixture.sent_keys())
        .await
        .expect("send_keys must not fail");
    let after_send = tmux
        .capture(&pane.id, 100)
        .await
        .expect("capture after send_keys must not fail");
    assert!(
        after_send.contains(fixture.sent_keys_capture()),
        "literal sent keys must become visible; expected {:?} in {after_send:?}",
        fixture.sent_keys_capture()
    );

    let mut staged = tmux
        .acquire_submit(&pane.id)
        .await
        .expect("acquire_submit must reserve the pane");
    tmux.stage_submit(&mut staged, "contract staged body")
        .await
        .expect("stage_submit must retain ownership");
    assert!(
        staged.staged,
        "successful staging updates the owned transaction"
    );
    tmux.commit_submit(&staged)
        .await
        .expect("commit_submit must submit and release ownership");
    let aborted = tmux
        .acquire_submit(&pane.id)
        .await
        .expect("a committed transaction releases the pane");
    assert_ne!(
        staged.token, aborted.token,
        "each transaction must receive a fresh ownership identity"
    );
    tmux.abort_submit(&aborted)
        .await
        .expect("abort_submit must release ownership without Enter");

    let created = tmux
        .new_window(&pane.session, fixture.new_window_name(), "/tmp", None)
        .await
        .expect("new_window must not fail");
    assert_eq!(
        created.session, pane.session,
        "new window stays in the fixture session"
    );
    assert_eq!(
        created.window,
        fixture.new_window_name(),
        "new window keeps its requested name"
    );

    tmux.kill(&created.id).await.expect("kill must not fail");
    let after_kill = tmux
        .list_panes()
        .await
        .expect("list after kill must not fail");
    assert!(
        after_kill
            .iter()
            .all(|candidate| candidate.id != created.id),
        "a killed pane must disappear from list_panes: {after_kill:?}"
    );
}
