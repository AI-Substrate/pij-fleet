//! Plan 188: a seat card is read from the seat's own rows, and the bulk read
//! (`GET /v1/cards`) is the single read (`POST /v1/state`) for every seat.
//!
//! The spine under the daemon is `CountingSpine::failing_on_tail`: any
//! `Spine::tail` on the card path is an error, not a slow call. On main every
//! card read three seat tails (card, state note, held deliveries), and on
//! SQLite each of those walked the whole spine.

use std::net::SocketAddr;
use std::sync::Arc;

use pij_core::delivery::{HeldEvent, ReleasedEvent};
use pij_core::model::{Envelope, Event, Harness, SeatDescriptor, SemanticState, Seq};
use pij_core::ports::Spine;
use pij_core::report::{CardRecord, StateRecord};
use pij_testkit::fakes::{CountingSpine, FakeQueue, FakeRegistry, FakeSpine};

use super::router_with_config;
use super::tests::{config, spawn, test_services};

const BUSY: &str = "pij-card-busy";
const QUIET: &str = "pij-card-quiet";
const GONE: &str = "pij-card-gone";

fn event(kind: &str, seat: &str, at: u64, payload: String) -> Event {
    Event {
        seq: None,
        v: 1,
        at,
        kind: kind.to_string(),
        seat: Some(seat.into()),
        payload,
    }
}

fn held(msg_id: &str) -> String {
    serde_json::to_string(&HeldEvent {
        msg_id: msg_id.to_string(),
        reason: "human-typing".to_string(),
        seat: BUSY.into(),
        since_ms: 5,
    })
    .expect("held")
}

/// What the busy seat's history holds, and the sequences a card must cite.
struct Fleet {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    spine: Arc<CountingSpine>,
    state_seq: Seq,
    card_seq: Seq,
}

/// Three seats: one with a card, a state note and held deliveries; one with
/// nothing; one tombstoned with a card. The state record is written BEFORE
/// the card, so a note that borrowed the card's time or cursor is visible.
async fn fleet() -> Fleet {
    let mut gone = SeatDescriptor::new(GONE, Harness::Claude, "/abs/gone");
    gone.tombstoned_at = Some(9);
    gone.tombstone_reason = Some("closed".to_string());
    let registry = Arc::new(
        FakeRegistry::new()
            .with_seat(SeatDescriptor::new(BUSY, Harness::Claude, "/abs/busy"))
            .with_seat(SeatDescriptor::new(QUIET, Harness::Omp, "/abs/quiet"))
            .with_seat(gone),
    );
    let mut services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let spine = Arc::new(CountingSpine::failing_on_tail());
    let append = |event: Event| {
        let spine = Arc::clone(&spine);
        async move { spine.append(event).await.expect("append") }
    };
    append(event("bg.lost", BUSY, 100, "{}".to_string())).await;
    append(event("delivery.held", BUSY, 110, held("m-released"))).await;
    append(event("delivery.held", BUSY, 120, held("m-still-held"))).await;
    let state_seq = append(event(
        "report.state",
        BUSY,
        130,
        serde_json::to_string(&StateRecord {
            state: Some(SemanticState::Blocked),
            note: Some("waiting on the seam".to_string()),
            assignment_id: None,
            refs: vec!["docs/x.md".to_string()],
            registry_seq: Seq(1),
        })
        .expect("state record"),
    ))
    .await;
    append(event(
        "delivery.released",
        BUSY,
        140,
        serde_json::to_string(&ReleasedEvent {
            at_ms: 140,
            msg_id: "m-released".to_string(),
            seat: BUSY.into(),
        })
        .expect("released"),
    ))
    .await;
    let card_seq = append(event(
        "report.now",
        BUSY,
        150,
        serde_json::to_string(&CardRecord {
            did: "landed the seam".to_string(),
            next: "review".to_string(),
        })
        .expect("card"),
    ))
    .await;
    append(event("bg.lost", BUSY, 160, "{}".to_string())).await;
    append(event(
        "report.now",
        GONE,
        170,
        serde_json::to_string(&CardRecord {
            did: "old".to_string(),
            next: "none".to_string(),
        })
        .expect("card"),
    ))
    .await;
    services.spine = Arc::new(crate::events::EventBus::new(spine.clone(), 16).expect("event bus"));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    Fleet {
        addr,
        server,
        spine,
        state_seq,
        card_seq,
    }
}

async fn state(addr: SocketAddr, seat: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "id": seat }))
        .send()
        .await
        .expect("state request");
    let status = response.status();
    let body = response.text().await.expect("state body");
    assert_eq!(status, reqwest::StatusCode::OK, "{seat}: {body}");
    let envelope: Envelope<serde_json::Value> = serde_json::from_str(&body).expect("envelope");
    envelope.data.expect("state data")
}

/// A card read cites the card, the state note and the held deliveries from
/// bounded reads only: no `Spine::tail`, which on SQLite was the whole spine.
#[tokio::test]
async fn a_card_read_never_tails_the_spine() {
    let fleet = fleet().await;

    let card = state(fleet.addr, BUSY).await;

    assert_eq!(fleet.spine.tail_calls(), 0, "a card read tailed the spine");
    assert_eq!(card["statusPrev"], "landed the seam");
    assert_eq!(card["statusSeq"], fleet.card_seq.0);
    assert_eq!(card["stateNote"], "waiting on the seam");
    assert_eq!(card["refs"], serde_json::json!(["docs/x.md"]));
    let held: Vec<&str> = card["held"]
        .as_array()
        .expect("held array")
        .iter()
        .map(|hold| hold["msg_id"].as_str().expect("msg_id"))
        .collect();
    assert_eq!(held, ["m-still-held"], "a released hold is not held");

    fleet.server.abort();
}

/// `stateNoteAt` and `stateNoteSeq` are the `report.state` record's own time
/// and cursor: never the card's, which is newer here.
#[tokio::test]
async fn the_state_note_cites_its_own_record() {
    let fleet = fleet().await;

    let card = state(fleet.addr, BUSY).await;

    assert_eq!(card["stateNoteAt"], 130);
    assert_eq!(card["stateNoteSeq"], fleet.state_seq.0);
    assert_ne!(card["stateNoteSeq"], card["statusSeq"]);

    let quiet = state(fleet.addr, QUIET).await;
    for key in ["stateNote", "stateNoteAt", "stateNoteSeq"] {
        assert!(
            quiet.get(key).is_some_and(serde_json::Value::is_null),
            "{key} is present and null for a seat with no note"
        );
    }

    fleet.server.abort();
}

/// One call returns every untombstoned seat's card, and each is exactly the
/// card `/v1/state` gives for that seat.
#[tokio::test]
async fn the_bulk_read_is_the_single_read_for_every_seat() {
    let fleet = fleet().await;

    let response = reqwest::Client::new()
        .get(format!("http://{}/v1/cards", fleet.addr))
        .bearer_auth("key")
        .send()
        .await
        .expect("cards request");
    let status = response.status();
    let body = response.text().await.expect("cards body");
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    let envelope: Envelope<serde_json::Value> = serde_json::from_str(&body).expect("envelope");
    assert_eq!(envelope.command, "pij cards");
    let cards = envelope.data.expect("cards data")["cards"]
        .as_array()
        .expect("cards array")
        .clone();

    let ids: Vec<&str> = cards
        .iter()
        .map(|card| card["id"].as_str().expect("id"))
        .collect();
    assert_eq!(
        ids,
        [BUSY, QUIET],
        "every untombstoned seat, in roster order"
    );
    for card in &cards {
        let id = card["id"].as_str().expect("id");
        assert_eq!(card, &state(fleet.addr, id).await, "{id}: bulk != single");
    }
    assert_eq!(
        fleet.spine.tail_calls(),
        0,
        "the bulk read tailed the spine"
    );

    fleet.server.abort();
}

/// The card's wire shape, pinned (plan 188, R4): every field a consumer
/// parses keeps its name and meaning, and the two new ones are additive. The
/// busy seat's card is deterministic: fixed event times, no session, no process.
#[tokio::test]
async fn the_card_wire_shape_matches_the_committed_golden() {
    let fleet = fleet().await;

    let card = state(fleet.addr, BUSY).await;

    let mut pretty = serde_json::to_string_pretty(&card).expect("card json");
    pretty.push('\n');
    pij_testkit::golden::assert_golden("api/state-card.json", &pretty);

    fleet.server.abort();
}
