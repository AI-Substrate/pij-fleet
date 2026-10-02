//! Wave-1 first light: one real registry round-trip and one event fan-out, on the
//! composed tree, through the real store.
//!
//! Not a unit test and not an exit code — a **transcript**. The cadence for this
//! wave says first light is "one real registry round-trip + one event fan-out",
//! so this runs the composed `Services` against a real SQLite file, narrates each
//! step to stdout, and asserts the claims it prints. Run it with:
//!
//! ```bash
//! cargo test -p pij-daemon --test first_light_wave1 -- --nocapture
//! ```
//!
//! The narration is the deliverable; the assertions are what stop the narration
//! from being fiction.

use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::events::EventFilter;
use pij_core::model::{Event, Harness, SeatDescriptor, SeatId, SemanticState, Seq};
use pij_core::ports::SeatFilter;
use pij_testkit::FreshStore;
use tokio_stream::StreamExt;

#[tokio::test]
async fn wave_1_first_light() {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            // Every port that HAS a real adapter after wave 1, made real.
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            liveness: AdapterChoice::Real,
            tmux: AdapterChoice::Fake, // real tmux would tap the operator's fleet
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };

    println!("\n=== wave-1 first light =========================================");
    println!("store            {}", store.path());
    println!(
        "adapters         registry/spine/queue/liveness REAL · tmux fake (a real\n\
         \x20                tmux here would tap the operator's live fleet)"
    );

    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-pane-signals"))
            .await
            .expect("the composed tree must build its services");
    println!(
        "offline          {} (a real store is not offline)",
        services.offline
    );
    assert!(!services.offline);

    // --- 1. Registry round-trip, through SQLite ----------------------------
    println!("\n--- 1. registry round-trip -------------------------------------");
    let mut seat = SeatDescriptor::new("pij-first-light", Harness::Omp, "/abs/tree");
    seat.semantic_state = Some(SemanticState::Ready);
    seat.parent = Some(SeatId::from("pij-revolutionary-lungfish"));

    let seq = services
        .registry
        .put(seat.clone())
        .await
        .expect("put must persist");
    println!("put              seq={seq:?}");

    let read_back = services
        .registry
        .get(&seat.id)
        .await
        .expect("get must not fail")
        .expect("the seat we just wrote must be present");
    println!(
        "get              {} · harness={} · parent={:?} · semantic={:?}",
        read_back.id, read_back.harness, read_back.parent, read_back.semantic_state
    );
    assert_eq!(read_back, seat, "what was written must read back identical");

    let listed = services
        .registry
        .list(SeatFilter {
            harness: Some(Harness::Omp),
            ..SeatFilter::default()
        })
        .await
        .expect("list must not fail");
    println!("list(harness=omp) {} row(s)", listed.len());
    assert_eq!(listed.len(), 1);

    services
        .registry
        .tombstone(&seat.id, "first light complete")
        .await
        .expect("tombstone must not fail");
    let dead = services
        .registry
        .get(&seat.id)
        .await
        .expect("get")
        .expect("a tombstoned seat is still readable — the row IS the post-mortem");
    println!(
        "tombstone        reason={:?} at={:?}",
        dead.tombstone_reason, dead.tombstoned_at
    );
    assert_eq!(
        dead.tombstone_reason.as_deref(),
        Some("first light complete"),
        "the reason must be readable THROUGH the port (u-store D3)"
    );

    // --- 2. Event fan-out, durable + live ----------------------------------
    println!("\n--- 2. event fan-out -------------------------------------------");
    let bus = Arc::clone(&services.event_bus);

    let event = |kind: &str| Event {
        seq: None,
        v: 1,
        at: 1_724_800_000_000,
        kind: kind.to_string(),
        seat: Some(seat.id.clone()),
        payload: "{}".to_string(),
    };

    let first = bus.publish(event("report")).await.expect("publish");
    println!("publish #1       seq={first:?} (durable leg: appended to the spine)");

    let tailed = services
        .spine
        .tail(None, Seq(0))
        .await
        .expect("tail must not fail");
    println!(
        "spine tail       {} event(s); kinds={:?}",
        tailed.len(),
        tailed.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
    assert!(
        tailed.iter().all(|e| e.seq.is_some()),
        "every tailed event carries the cursor a consumer needs (the wave-1 seam)"
    );

    let cursor = tailed.last().and_then(|e| e.seq).expect("a tailed cursor");
    println!("cursor           {cursor:?} — a subscriber resumes from here");

    // Two subscribers attaching AT the cursor, then one publish: fan-out means
    // both get it, and neither is handed the history it already has.
    //
    // `since: None` would replay from the very start — documented on `subscribe`,
    // and correct for a cursor that has seen nothing, but it is not what a
    // subscriber joining a running fleet wants. Passing the cursor is the join
    // this whole seam exists for; there is no gap, because the live receiver is
    // created before the tail is read.
    let mut a = bus
        .subscribe(Some(cursor), EventFilter::all())
        .await
        .expect("subscribe a");
    let mut b = bus
        .subscribe(Some(cursor), EventFilter::all())
        .await
        .expect("subscribe b");
    let second = bus.publish(event("receipt")).await.expect("publish");
    println!("publish #2       seq={second:?} to 2 live subscribers");

    let got_a = a.next().await.expect("subscriber a receives");
    let got_b = b.next().await.expect("subscriber b receives");
    println!("subscriber a     kind={} seq={:?}", got_a.kind, got_a.seq);
    println!("subscriber b     kind={} seq={:?}", got_b.kind, got_b.seq);
    assert_eq!(got_a.kind, "receipt");
    assert_eq!(
        got_a, got_b,
        "fan-out means every subscriber sees the SAME event"
    );
    assert_eq!(
        a.dropped_count(),
        0,
        "nothing was dropped for a reader that kept up"
    );

    // The join a late subscriber makes: replay from the cursor, then live.
    let replayed = services
        .spine
        .tail(None, cursor)
        .await
        .expect("replay from the cursor");
    println!(
        "replay(> {cursor:?})   {} event(s) — the exactly-once join between what a\n\
         \x20                subscriber missed and what it now receives",
        replayed.len()
    );
    assert_eq!(
        replayed.len(),
        1,
        "exactly the event published after the cursor"
    );
    assert_eq!(replayed[0].kind, "receipt");

    println!("\n=== first light complete ======================================\n");
}
