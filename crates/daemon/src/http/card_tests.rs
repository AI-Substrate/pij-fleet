//! Plan 188: a seat card is read from the seat's own rows, and the bulk read
//! (`GET /v1/cards`) is the single read (`POST /v1/state`) for every seat.
//!
//! The spine under the daemon is `CountingSpine::failing_on_tail`: any
//! `Spine::tail` on the card path is an error, not a slow call. On main every
//! card read three seat tails (card, state note, held deliveries), and on
//! SQLite each of those walked the whole spine.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pij_core::delivery::{HeldEvent, ReleasedEvent};
use pij_core::error::Result;
use pij_core::model::{Envelope, Event, Harness, SeatDescriptor, SemanticState, Seq};
use pij_core::ports::{SessionStatusPort, Spine};
use pij_core::report::{CardRecord, StateRecord};
use pij_core::session_status::{SessionStatusReply, SessionTarget};
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

// ─── The bulk read on a larger fleet (review of #105: S1, S2, N2) ───────────

/// How a fleet seat's session read behaves.
#[derive(Clone, Copy)]
enum Session {
    /// No bound session: the card says `unbound` without asking the source.
    Unbound,
    /// The source answers after this long.
    After(Duration),
    /// The source never answers.
    Never,
}

/// A session source scripted per seat.
struct ScriptedSessions(HashMap<String, Session>);

#[async_trait]
impl SessionStatusPort for ScriptedSessions {
    async fn status(&self, target: &SessionTarget) -> Result<SessionStatusReply> {
        match self.0.get(target.seat.as_str()) {
            Some(Session::Never) => std::future::pending().await,
            Some(Session::After(delay)) => tokio::time::sleep(*delay).await,
            _ => {}
        }
        Ok(SessionStatusReply::NotFound {
            detail: format!("scripted {}", target.session),
        })
    }
}

struct FleetSeat {
    id: &'static str,
    session: Session,
    role: Option<&'static str>,
    holds: usize,
    corrupt_card: bool,
}

const fn seat(id: &'static str) -> FleetSeat {
    FleetSeat {
        id,
        session: Session::Unbound,
        role: None,
        holds: 0,
        corrupt_card: false,
    }
}

/// A daemon over `seats`, each with a card (or a corrupt one), its holds,
/// its role assertion and its scripted session read.
async fn bulk_fleet(seats: &[FleetSeat]) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let mut registry = FakeRegistry::new();
    for seat in seats {
        let mut descriptor = SeatDescriptor::new(seat.id, Harness::Claude, "/abs/fleet");
        if !matches!(seat.session, Session::Unbound) {
            descriptor.harness_session = Some(format!("session-{}", seat.id));
        }
        registry = registry.with_seat(descriptor);
    }
    let mut services = test_services(
        Arc::new(registry),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.session_status = Arc::new(ScriptedSessions(
        seats
            .iter()
            .map(|seat| (seat.id.to_string(), seat.session))
            .collect(),
    ));
    let spine = Arc::new(CountingSpine::failing_on_tail());
    for (at, seat) in (1_000_u64..).step_by(100).zip(seats) {
        let payload = if seat.corrupt_card {
            "not a card".to_string()
        } else {
            serde_json::to_string(&CardRecord {
                did: format!("{} did", seat.id),
                next: "next".to_string(),
            })
            .expect("card")
        };
        spine
            .append(event("report.now", seat.id, at, payload))
            .await
            .expect("card");
        for hold in 0..seat.holds {
            let held = HeldEvent {
                msg_id: format!("m-{}-{hold}", seat.id),
                reason: "human-typing".to_string(),
                seat: seat.id.into(),
                since_ms: at,
            };
            spine
                .append(event(
                    "delivery.held",
                    seat.id,
                    at + 1,
                    serde_json::to_string(&held).expect("held"),
                ))
                .await
                .expect("hold");
        }
        if let Some(role) = seat.role {
            services
                .roles
                .assert_role(&seat.id.into(), &seat.id.into(), Some(role.to_string()))
                .await
                .expect("assert role");
        }
    }
    services.spine = Arc::new(crate::events::EventBus::new(spine, 16).expect("event bus"));
    spawn(router_with_config(services, config("key", &[]))).await
}

async fn cards(addr: SocketAddr) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/cards"))
        .bearer_auth("key")
        .send()
        .await
        .expect("cards request");
    let status = response.status();
    let body = response.text().await.expect("cards body");
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    let envelope: Envelope<serde_json::Value> = serde_json::from_str(&body).expect("envelope");
    envelope.data.expect("cards data")
}

fn ids(cards: &serde_json::Value) -> Vec<String> {
    cards
        .as_array()
        .expect("array")
        .iter()
        .map(|card| card["id"].as_str().expect("id").to_string())
        .collect()
}

/// N2: five seats, the FIRST the slowest to build and one with an asserted
/// role. The bulk read keeps seat-id order whatever order the cards finish in,
/// joins roles exactly as `/v1/state` projects them, and each card equals the
/// single read.
#[tokio::test]
async fn the_bulk_read_keeps_roster_order_and_roles_on_a_wider_fleet() {
    let (addr, server) = bulk_fleet(&[
        FleetSeat {
            session: Session::After(Duration::from_millis(40)),
            holds: 3,
            ..seat("pij-fleet-a")
        },
        FleetSeat {
            role: Some("prime"),
            ..seat("pij-fleet-b")
        },
        seat("pij-fleet-c"),
        seat("pij-fleet-d"),
        seat("pij-fleet-e"),
    ])
    .await;

    let data = cards(addr).await;

    assert_eq!(
        ids(&data["cards"]),
        [
            "pij-fleet-a",
            "pij-fleet-b",
            "pij-fleet-c",
            "pij-fleet-d",
            "pij-fleet-e"
        ],
        "seat-id order, not the order the cards finished in"
    );
    assert_eq!(
        data["cards"][1]["role"], "prime",
        "the role join reached the bulk read"
    );
    assert_eq!(data["unavailable"], serde_json::json!([]));
    for card in data["cards"].as_array().expect("cards") {
        let id = card["id"].as_str().expect("id");
        assert_eq!(card, &state(addr, id).await, "{id}: bulk != single");
    }

    server.abort();
}

/// S1: the bulk read bounds each seat's session read. A seat whose transcript
/// read never returns does not hold the response, and its card says why.
#[tokio::test]
async fn a_session_read_that_never_returns_does_not_hold_the_bulk_read() {
    let (addr, server) = bulk_fleet(&[
        FleetSeat {
            session: Session::Never,
            ..seat("pij-fleet-hung")
        },
        seat("pij-fleet-ok"),
    ])
    .await;

    let started = std::time::Instant::now();
    let data = tokio::time::timeout(Duration::from_secs(5), cards(addr))
        .await
        .expect("the bulk read waited on a session read that never returns");

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(ids(&data["cards"]), ["pij-fleet-hung", "pij-fleet-ok"]);
    assert_eq!(
        data["cards"][0]["sessionStatus"],
        serde_json::json!({"outcome": "failed", "error": "no answer within 200 ms"})
    );
    assert_eq!(data["cards"][0]["statusPrev"], "pij-fleet-hung did");
    assert_eq!(data["cards"][1]["sessionStatus"]["outcome"], "unbound");

    server.abort();
}

/// S2: one seat whose card cannot be read is named in `unavailable`; every
/// other seat's card is still served.
#[tokio::test]
async fn one_unreadable_seat_is_named_and_the_fleet_is_still_served() {
    let (addr, server) = bulk_fleet(&[
        FleetSeat {
            corrupt_card: true,
            ..seat("pij-fleet-bad")
        },
        seat("pij-fleet-good"),
    ])
    .await;

    let data = cards(addr).await;

    assert_eq!(ids(&data["cards"]), ["pij-fleet-good"]);
    let unavailable = data["unavailable"].as_array().expect("unavailable");
    assert_eq!(unavailable.len(), 1, "{unavailable:?}");
    assert_eq!(unavailable[0]["id"], "pij-fleet-bad");
    let error = unavailable[0]["error"].as_str().expect("error");
    assert!(error.contains("report.now"), "{error}");

    server.abort();
}

/// Recheck of #105 (R1): only the bulk read is bounded. A session that answers
/// after 300 ms is a timeout in `/v1/cards` and the full answer in `/v1/state`,
/// which waits for the read as it always has.
#[tokio::test]
async fn only_the_bulk_read_bounds_the_session_read() {
    let (addr, server) = bulk_fleet(&[FleetSeat {
        session: Session::After(Duration::from_millis(300)),
        ..seat("pij-fleet-slow")
    }])
    .await;

    let bulk = cards(addr).await;
    assert_eq!(
        bulk["cards"][0]["sessionStatus"],
        serde_json::json!({"outcome": "failed", "error": "no answer within 200 ms"})
    );

    let started = std::time::Instant::now();
    let single = state(addr, "pij-fleet-slow").await;
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(single["sessionStatus"]["outcome"], "not-found", "{single}");
    assert_eq!(
        single["sessionStatus"]["detail"],
        "scripted session-pij-fleet-slow"
    );

    server.abort();
}
