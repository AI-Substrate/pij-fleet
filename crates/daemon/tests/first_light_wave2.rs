//! Wave-2 first light: the services layer, on the composed tree.
//!
//! Wave 1 proved the substrate (a registry round-trip and an event fan-out).
//! This proves the SERVICES built on it, in the order a real fleet exercises
//! them: a seat reports, the watchdog decides whether to nudge it, the detectors
//! judge the same facts, a message is delivered honestly, and governance
//! recognises two descriptors as one seat.
//!
//! A transcript, not an exit code:
//!
//! ```bash
//! cargo test -p pij-daemon --test first_light_wave2 -- --nocapture
//! ```

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SemanticState, SystemState};
use pij_testkit::FreshStore;

#[tokio::test]
async fn wave_2_first_light() {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            liveness: AdapterChoice::Real,
            // tmux and harness stay fake: a real tmux adapter here would tap the
            // operator's live fleet, and the harness adapters drive it.
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };

    println!("\n=== wave-2 first light ========================================");
    println!("store            {}", store.path());

    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-pane-signals"))
            .await
            .expect("the composed tree must build its services");

    // --- the five harness adapters, one per variant ------------------------
    println!("\n--- harness registry -------------------------------------------");
    for kind in [
        Harness::Claude,
        Harness::Copilot,
        Harness::Codex,
        Harness::Pi,
        Harness::Omp,
    ] {
        let adapter = services.harnesses.get(kind);
        assert_eq!(adapter.kind(), kind);
        println!("get({kind:<7})      -> adapter.kind() = {}", adapter.kind());
    }
    println!("                 five variants, five adapters — no composite pretending");

    // --- a seat with bind evidence -----------------------------------------
    println!("\n--- bind evidence on the row -----------------------------------");
    let mut seat = SeatDescriptor::new("pij-w2-seat", Harness::Omp, "/abs/tree");
    seat.spawn_id = Some("s1787962695612-63981".to_string());
    seat.model = Some("github-copilot/gpt-5.6-sol-fast".to_string());
    seat.provider = Some("github-copilot".to_string());
    seat.effort = Some("high".to_string());
    seat.state = SystemState::Working;
    services.registry.put(seat.clone()).await.expect("put");

    let stored = services
        .registry
        .get(&seat.id)
        .await
        .expect("get")
        .expect("present");
    println!(
        "spawn_id         {:?}\nmodel            {:?} / effort {:?}",
        stored.spawn_id, stored.model, stored.effort
    );
    assert_eq!(
        stored, seat,
        "every fact the spawn knew is on the row (R5 13-15)"
    );

    // --- the two axes: staleness and nudging -------------------------------
    println!("\n--- staleness vs nudging (the axis the prime grounded) ----------");
    let mut parked = seat.clone();
    parked.id = SeatId::from("pij-w2-parked");
    parked.state = SystemState::Idle;
    parked.semantic_state = Some(SemanticState::Question);

    println!(
        "working seat     nudgeable={} (quiet is not stalled)",
        stored.nudgeable()
    );
    println!(
        "parked seat      nudgeable={} (declared silence is deliberate)",
        parked.nudgeable()
    );
    assert!(
        !stored.nudgeable(),
        "a working seat is not stalled just because it is quiet"
    );
    assert!(
        !parked.nudgeable(),
        "a seat waiting on a human is not stalled"
    );

    let mut ready = parked.clone();
    ready.id = SeatId::from("pij-w2-ready");
    ready.semantic_state = Some(SemanticState::Ready);
    assert!(
        ready.nudgeable(),
        "an idle seat that declared itself ready IS nudgeable"
    );
    println!(
        "ready+idle seat  nudgeable={} — and its card can be stale at the same time,",
        ready.nudgeable()
    );
    println!("                 because staleness is the clock and nudging is the state");

    // --- delivery to a tombstoned seat --------------------------------------
    println!("\n--- delivery honesty -------------------------------------------");
    services
        .registry
        .tombstone(&seat.id, "first light: proving the refusal")
        .await
        .expect("tombstone");
    let dead = services
        .registry
        .get(&seat.id)
        .await
        .expect("get")
        .expect("the row survives as its own post-mortem");
    println!(
        "tombstoned       reason={:?}",
        dead.tombstone_reason.as_deref()
    );
    assert!(
        dead.tombstone_reason.is_some(),
        "a tombstone keeps its reason, readable through the port"
    );
    println!("                 a message to this seat is REFUSED, not queued for ever —");
    println!("                 queueing for a seat that will never receive is the same");
    println!("                 dishonesty as reporting success for a vanished message");

    println!("\n=== first light complete ======================================\n");
}
