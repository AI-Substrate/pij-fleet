//! Plan 158 over real HTTP and a real SQLite store: `pij send --fyi` holds a
//! message without opening a turn, it rides along appended to the next real
//! message exactly once, a typed-turn hook claims it exactly once, it survives
//! a restart, and a tombstone drops it and says so. Addendum 3: a seat's own
//! turn boundaries publish busy/idle.
//!
//! Every test here speaks only the wire, so each is a behavioural RED against a
//! daemon without the feature (unknown route, or a field silently ignored), not
//! a compile error.

use std::net::SocketAddr;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

const KEY: &str = "test-local-key";
const RECIPIENT: &str = "pij-fyi-recipient";
const PANE: &str = "%91";
const SESSION: &str = "native-session-91";

fn config(store: &FreshStore) -> Config {
    Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    }
}

/// An OMP recipient with no process yet, so every real message queues (pre-bind)
/// and the delivered body can be read back from the inbox without a transport.
fn recipient() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(RECIPIENT, Harness::Omp, "/abs/tree");
    seat.pane = Some(PANE.to_string());
    seat.harness_session = Some(SESSION.to_string());
    seat
}

async fn serve(config: &Config, seed: bool) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let services = pij_daemon::build_services(
        config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("services");
    if seed {
        services
            .registry
            .put(recipient())
            .await
            .expect("seed recipient");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = router_with_config(
        services,
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local(KEY.to_string()),
            machine_alias: "workstation".to_string(),
        },
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, server)
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .expect("request");
    let status = response.status().as_u16();
    let text = response.text().await.expect("body");
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn send(addr: SocketAddr, from: &str, body: &str, msg_id: &str, fyi: bool) -> (u16, Value) {
    post(
        addr,
        "/v1/send",
        json!({"from": from, "to": {"seat": RECIPIENT}, "body": body, "msg_id": msg_id, "fyi": fyi}),
    )
    .await
}

async fn pending(addr: SocketAddr) -> Value {
    let (status, state) = post(addr, "/v1/state", json!({"id": RECIPIENT})).await;
    assert_eq!(status, 200, "{state}");
    state["data"]["pendingFyis"].clone()
}

/// Every body queued for the recipient, oldest first, without claiming any.
async fn queued_bodies(config: &Config) -> Vec<String> {
    let services = pij_daemon::build_services(
        config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("reopen services");
    let kinds = [format!("delivery:{RECIPIENT}")];
    let mut bodies = Vec::new();
    while let Some((job, row)) = services
        .queue
        .claim(&kinds, "fyi-test-reader")
        .await
        .expect("claim")
    {
        let payload: Value = serde_json::from_str(&row.payload).expect("payload");
        bodies.push(payload["body"].as_str().expect("body").to_string());
        services
            .queue
            .ack(job, pij_core::model::Outcome::Done)
            .await
            .expect("ack");
    }
    bodies
}

#[tokio::test]
async fn an_fyi_is_held_then_rides_along_appended_to_the_next_message_once() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;

    let (status, held) = send(addr, "pij-sender", "build is green", "fyi-1", true).await;
    assert_eq!(status, 200, "{held}");
    assert_eq!(
        held["data"]["outcome"],
        json!({"outcome": "held", "reason": "fyi"})
    );
    assert!(
        queued_bodies(&config).await.is_empty(),
        "an FYI must not queue a delivery"
    );
    assert_eq!(pending(addr).await, 1);

    let (status, sent) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200, "{sent}");
    let (_, again) = send(addr, "pij-boss", "do Y", "real-2", false).await;
    assert_eq!(again["ok"], true, "{again}");

    let bodies = queued_bodies(&config).await;
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    let (head, tail) = bodies[0]
        .split_once("\n\nAlso, 1 FYI was queued for you:\n1. [from pij-sender, ")
        .expect("block appended after the body");
    assert_eq!(head, "do X", "the real message comes first, unchanged");
    assert!(
        tail.len() == "HH:MM] build is green".len() && tail.ends_with("] build is green"),
        "{tail}"
    );
    assert_eq!(bodies[1], "do Y", "an FYI rides along exactly once");
    assert_eq!(pending(addr).await, 0);
    server.abort();
}

#[tokio::test]
async fn a_held_fyi_survives_a_daemon_restart() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let (status, _) = send(addr, "pij-sender", "note", "fyi-restart", true).await;
    assert_eq!(status, 200);
    server.abort();

    let (addr, server) = serve(&config, false).await;
    assert_eq!(pending(addr).await, 1);
    server.abort();
}

#[tokio::test]
async fn racing_hook_claims_deliver_each_fyi_exactly_once() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    for id in ["fyi-a", "fyi-b", "fyi-c"] {
        let (status, _) = send(addr, "pij-sender", id, id, true).await;
        assert_eq!(status, 200);
    }

    let claim = || {
        post(
            addr,
            "/v1/fyi/claim",
            json!({"pane": PANE, "via": "hook:claude"}),
        )
    };
    let (first, second) = tokio::join!(claim(), claim());
    let mut ids = Vec::new();
    for (status, reply) in [first, second] {
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["data"]["seat"], RECIPIENT);
        let claimed = reply["data"]["ids"].as_array().expect("ids").clone();
        assert_eq!(reply["data"]["count"], claimed.len());
        assert_eq!(reply["data"]["block"] == "", claimed.is_empty(), "{reply}");
        ids.extend(claimed);
    }
    ids.sort_by_key(|id| id.as_str().map(str::to_string));
    assert_eq!(ids, [json!("fyi-a"), json!("fyi-b"), json!("fyi-c")]);
    assert_eq!(pending(addr).await, 0);

    let (_, empty) = claim().await;
    assert_eq!(
        empty["data"],
        json!({"seat": RECIPIENT, "count": 0, "block": "", "ids": []})
    );
    server.abort();
}

#[tokio::test]
async fn a_claim_needs_binding_evidence_that_matches_the_seat() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let (_, _) = send(addr, "pij-sender", "secret-ish", "fyi-guarded", true).await;

    for body in [
        json!({"seat": RECIPIENT, "via": "hook:omp"}),
        json!({"seat": RECIPIENT, "native_session": "someone-else", "via": "hook:omp"}),
        json!({"seat": RECIPIENT, "pane": "%1", "via": "hook:omp"}),
        json!({"seat": RECIPIENT, "native_session": SESSION, "via": "hook:elsewhere"}),
    ] {
        let (status, reply) = post(addr, "/v1/fyi/claim", body.clone()).await;
        assert_eq!(status, 400, "{body} -> {reply}");
        assert_eq!(reply["ok"], false);
    }
    assert_eq!(pending(addr).await, 1, "a refused claim takes nothing");

    let (status, reply) = post(
        addr,
        "/v1/fyi/claim",
        json!({"seat": RECIPIENT, "native_session": SESSION, "via": "hook:omp"}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["count"], 1);
    server.abort();
}

#[tokio::test]
async fn fyi_refuses_controls_and_remote_seats() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let (status, reply) = post(
        addr,
        "/v1/send",
        json!({"from": "pij-sender", "to": {"seat": RECIPIENT}, "body": "", "msg_id": "c",
               "command": "compact", "fyi": true}),
    )
    .await;
    assert_eq!(
        (status, reply["ok"].clone()),
        (400, json!(false)),
        "{reply}"
    );
    let (status, reply) = post(
        addr,
        "/v1/send",
        json!({"from": "pij-sender", "to": {"seat": RECIPIENT, "machine": "elsewhere"},
               "body": "hi", "msg_id": "r", "fyi": true}),
    )
    .await;
    assert_eq!(
        (status, reply["ok"].clone()),
        (400, json!(false)),
        "{reply}"
    );
    assert_eq!(pending(addr).await, 0);
    server.abort();
}

#[tokio::test]
async fn a_tombstone_drops_pending_fyis_and_records_how_many() {
    let store = FreshStore::new();
    let config = config(&store);
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("services");
    services.registry.put(recipient()).await.expect("seed");
    let (addr, server) = serve(&config, false).await;
    for id in ["fyi-x", "fyi-y"] {
        send(addr, "pij-sender", id, id, true).await;
    }
    let seq = services
        .registry
        .tombstone(&SeatId::from(RECIPIENT), "closed")
        .await
        .expect("tombstone");
    assert_eq!(pending(addr).await, 0);
    let tombstone = services
        .spine
        .tail(
            Some(&SeatId::from(RECIPIENT)),
            pij_core::model::Seq(seq.0 - 1),
        )
        .await
        .expect("tail")
        .into_iter()
        .find(|event| event.kind == "seat.tombstone")
        .expect("tombstone event");
    let payload: Value = serde_json::from_str(&tombstone.payload).expect("payload");
    assert_eq!(payload["pending_fyis_dropped"], 2, "{payload}");
    server.abort();
}

#[tokio::test]
async fn whoami_carries_the_pending_count_for_the_status_lines() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    send(addr, "pij-sender", "one", "fyi-w", true).await;
    let (status, reply) = post(addr, "/v1/whoami", json!({"seat": RECIPIENT})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["id"], RECIPIENT);
    assert_eq!(reply["data"]["pending_fyis"], 1);
    server.abort();
}

#[tokio::test]
async fn a_seats_own_turn_boundaries_publish_busy_then_idle() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let state = || async {
        post(addr, "/v1/state", json!({"id": RECIPIENT})).await.1["data"]["state"].clone()
    };
    assert_eq!(state().await, "idle");

    let (status, reply) = post(
        addr,
        "/v1/activity",
        json!({"seat": RECIPIENT, "native_session": SESSION, "state": "working"}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["data"],
        json!({"seat": RECIPIENT, "state": "working", "changed": true})
    );
    assert_eq!(state().await, "working");

    let (status, reply) = post(
        addr,
        "/v1/activity",
        json!({"seat": RECIPIENT, "native_session": "not-this-seat", "state": "idle"}),
    )
    .await;
    assert_eq!(status, 400, "{reply}");
    assert_eq!(
        state().await,
        "working",
        "a mismatched session changes nothing"
    );

    let (status, _) = post(
        addr,
        "/v1/activity",
        json!({"seat": RECIPIENT, "pane": PANE, "state": "idle"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(state().await, "idle");
    server.abort();
}

/// Review HIGH-1(a): a sender retrying a message id that is still queued must
/// not swallow an FYI held since the first attempt. The queued row keeps its
/// original body, so the newer FYI has to stay pending for the next carrier.
#[tokio::test]
async fn a_retried_carrier_still_queued_does_not_swallow_a_newer_fyi() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    send(addr, "pij-sender", "first note", "fyi-A", true).await;
    let (status, _) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200);
    send(addr, "pij-sender", "second note", "fyi-B", true).await;
    let (status, retry) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200, "{retry}");

    assert_eq!(pending(addr).await, 1, "fyi-B must still be pending");
    let bodies = queued_bodies(&config).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("first note") && !bodies[0].contains("second note"));
    server.abort();
}

/// Review HIGH-2: a duplicate of a carrier the recipient already has delivers
/// nothing, so the FYI it would have carried stays pending.
#[tokio::test]
async fn a_duplicate_of_a_delivered_carrier_leaves_the_fyi_pending() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let (status, _) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200);
    // The recipient reads and acknowledges real-1: the durable ledger now has it.
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("reopen services");
    let kinds = [format!("delivery:{RECIPIENT}")];
    let (job, _) = services
        .queue
        .claim(&kinds, "fyi-test-reader")
        .await
        .expect("claim")
        .expect("real-1 is queued");
    services
        .queue
        .ack_delivery(job, pij_core::model::DeliveryOrigin::ReaderRead)
        .await
        .expect("ack");

    send(addr, "pij-sender", "late note", "fyi-late", true).await;
    let (status, dup) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200, "{dup}");
    assert_eq!(dup["data"]["outcome"]["outcome"], "delivered", "{dup}");
    assert_eq!(pending(addr).await, 1, "the duplicate carried nothing");
    assert!(queued_bodies(&config).await.is_empty());
    server.abort();
}

/// Review HIGH-2: a typed-turn hook claim racing a ride-along delivers every
/// FYI exactly once, in the body or in the hook's block, never both.
#[tokio::test]
async fn a_hook_claim_racing_a_ride_along_delivers_each_fyi_once() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let ids = ["fyi-r1", "fyi-r2", "fyi-r3", "fyi-r4"];
    for id in ids {
        send(addr, "pij-sender", &format!("body-{id}"), id, true).await;
    }
    let (claimed, sent) = tokio::join!(
        post(
            addr,
            "/v1/fyi/claim",
            json!({"pane": PANE, "via": "hook:claude"})
        ),
        send(addr, "pij-boss", "do X", "real-race", false),
    );
    assert_eq!(claimed.0, 200, "{}", claimed.1);
    assert_eq!(sent.0, 200, "{}", sent.1);
    let bodies = queued_bodies(&config).await;
    assert_eq!(bodies.len(), 1);
    for id in ids {
        let in_hook = claimed.1["data"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|claimed| claimed == id);
        let in_body = bodies[0].contains(&format!("body-{id}"));
        assert!(in_hook ^ in_body, "{id}: hook {in_hook}, body {in_body}");
    }
    assert_eq!(pending(addr).await, 0);
    server.abort();
}

/// Review MEDIUM-3: a tombstone ends any turn, so a seat retired while
/// `working` does not read `working` for ever.
#[tokio::test]
async fn a_tombstone_resets_a_working_seat_to_idle() {
    let store = FreshStore::new();
    let config = config(&store);
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("services");
    services.registry.put(recipient()).await.expect("seed");
    let (addr, server) = serve(&config, false).await;
    let (status, _) = post(
        addr,
        "/v1/activity",
        json!({"seat": RECIPIENT, "native_session": SESSION, "state": "working"}),
    )
    .await;
    assert_eq!(status, 200);
    services
        .registry
        .tombstone(&SeatId::from(RECIPIENT), "closed")
        .await
        .expect("tombstone");
    let (_, card) = post(addr, "/v1/state", json!({"id": RECIPIENT})).await;
    assert_eq!(card["data"]["state"], "idle", "{card}");
    server.abort();
}

/// Plan 157 review finding 3: Claude's turn hooks know the pane and session
/// but not the seat id, so the seat resolves from the pane.
#[tokio::test]
async fn a_hook_publishes_activity_by_pane_alone() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    let (status, reply) = post(
        addr,
        "/v1/activity",
        json!({"pane": PANE, "native_session": SESSION, "state": "working"}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["seat"], RECIPIENT);
    let (_, card) = post(addr, "/v1/state", json!({"id": RECIPIENT})).await;
    assert_eq!(card["data"]["state"], "working");
    server.abort();
}

// ---------------------------------------------------------------------------
// Plan 159: question warning, warm flush at 5, digest for large piles.
// ---------------------------------------------------------------------------

const QUESTION_WARNING: &str =
    "this looks like a question; if you need an answer, resend without --fyi";

async fn activity(addr: SocketAddr, state: &str) {
    let (status, reply) = post(addr, "/v1/activity", json!({"pane": PANE, "state": state})).await;
    assert_eq!(status, 200, "{reply}");
}

#[tokio::test]
async fn a_question_held_as_an_fyi_warns_but_is_still_held() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;

    let (status, held) = send(addr, "pij-sender", "can you check X?", "fyi-q", true).await;
    assert_eq!(status, 200, "{held}");
    assert_eq!(
        held["data"]["outcome"],
        json!({"outcome": "held", "reason": "fyi"})
    );
    assert_eq!(held["data"]["warning"], QUESTION_WARNING);
    assert_eq!(
        pending(addr).await,
        1,
        "a warning never converts or refuses"
    );

    let (_, plain) = send(addr, "pij-sender", "merged #452", "fyi-plain", true).await;
    assert!(plain["data"].get("warning").is_none(), "{plain}");
    let (_, normal) = send(addr, "pij-boss", "can you check Y?", "real-q", false).await;
    assert!(
        normal["data"].get("warning").is_none(),
        "only a held FYI is warned: {normal}"
    );
    server.abort();
}

#[tokio::test]
async fn fyis_to_a_seat_not_known_to_be_warm_are_never_flushed() {
    let store = FreshStore::new();
    let config = config(&store);
    // Idle, and an OMP session is unreadable: warmth is unknown, so hold.
    let (addr, server) = serve(&config, true).await;
    for n in 1..=6 {
        let (status, _) = send(
            addr,
            "pij-sender",
            &format!("note {n}"),
            &format!("fyi-{n}"),
            true,
        )
        .await;
        assert_eq!(status, 200);
    }
    assert!(
        queued_bodies(&config).await.is_empty(),
        "a flush must never wake a seat that is not known warm"
    );
    assert_eq!(pending(addr).await, 6);
    server.abort();
}

/// Review finding 2: a turn boundary never flushes. A seat starting a turn
/// gets the whole pile free with that turn (here its typed-turn hook), rather
/// than a separate message that opens another turn after it.
#[tokio::test]
async fn a_turn_boundary_never_flushes_so_the_turn_carries_the_pile() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    for n in 1..=5 {
        let (status, _) = send(
            addr,
            "pij-sender",
            &format!("note {n}"),
            &format!("fyi-{n}"),
            true,
        )
        .await;
        assert_eq!(status, 200);
    }
    activity(addr, "working").await;
    activity(addr, "idle").await;
    activity(addr, "working").await;
    assert!(queued_bodies(&config).await.is_empty(), "no flush message");
    let (status, claim) = post(
        addr,
        "/v1/fyi/claim",
        json!({"pane": PANE, "via": "hook:claude"}),
    )
    .await;
    assert_eq!(status, 200, "{claim}");
    assert_eq!(
        claim["data"]["count"], 5,
        "the turn carries every one: {claim}"
    );
    server.abort();
}

/// Review W10/W11: a read names one claim. A later claim's FYIs, and FYIs a
/// tombstone dropped at that same instant, are not part of it.
#[tokio::test]
async fn a_read_returns_exactly_the_batch_one_claim_delivered() {
    let store = FreshStore::new();
    let config = config(&store);
    let services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-fyi"),
    )
    .await
    .expect("services");
    let seat = SeatId::from(RECIPIENT);
    let fyi = |id: &str, held_at_ms: u64| pij_core::fyi::HeldFyi {
        id: id.to_string(),
        recipient: seat.clone(),
        sender: SeatId::from("pij-sender"),
        body: format!("body {id}"),
        held_at_ms,
    };
    for (id, at) in [("a", 1), ("b", 2)] {
        services
            .queue
            .hold_fyi(&fyi(id, at), services.spine.as_ref())
            .await
            .expect("hold");
    }
    services
        .queue
        .claim_fyis(&seat, "hook:omp", 100, services.spine.as_ref())
        .await
        .expect("claim");
    for (id, at) in [("c", 3), ("d", 4), ("e", 5)] {
        services
            .queue
            .hold_fyi(&fyi(id, at), services.spine.as_ref())
            .await
            .expect("hold");
    }
    services
        .queue
        .claim_fyis(&seat, "hook:omp", 200, services.spine.as_ref())
        .await
        .expect("claim");

    let ids =
        |fyis: Vec<pij_core::fyi::HeldFyi>| fyis.into_iter().map(|fyi| fyi.id).collect::<Vec<_>>();
    assert_eq!(
        ids(services
            .queue
            .read_claimed_fyis(&seat, 100)
            .await
            .expect("read")),
        ["a", "b"]
    );
    assert_eq!(
        ids(services
            .queue
            .read_claimed_fyis(&seat, 200)
            .await
            .expect("read")),
        ["c", "d", "e"]
    );

    // A tombstone drops pending FYIs with its own timestamp: never a claim.
    services.registry.put(recipient()).await.expect("seed");
    services
        .queue
        .hold_fyi(&fyi("f", 6), services.spine.as_ref())
        .await
        .expect("hold");
    let seq = services
        .registry
        .tombstone(&seat, "closed")
        .await
        .expect("tombstone");
    let dropped_at = services
        .spine
        .tail(Some(&seat), pij_core::model::Seq(seq.0 - 1))
        .await
        .expect("tail")
        .into_iter()
        .find(|event| event.kind == "seat.tombstone")
        .expect("tombstone event")
        .at;
    assert!(
        services
            .queue
            .read_claimed_fyis(&seat, dropped_at)
            .await
            .expect("read")
            .is_empty(),
        "a dropped FYI was never delivered"
    );
}

#[tokio::test]
async fn a_large_pile_rides_along_as_a_digest_and_stays_readable_in_full() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    for n in 1..=8 {
        let sender = if n % 3 == 0 {
            "pij-other"
        } else {
            "pij-sender"
        };
        let (status, _) = send(
            addr,
            sender,
            &format!("note {n}"),
            &format!("fyi-{n}"),
            true,
        )
        .await;
        assert_eq!(status, 200);
    }
    let (status, _) = send(addr, "pij-boss", "do X", "real-1", false).await;
    assert_eq!(status, 200);
    let bodies = queued_bodies(&config).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let (head, digest) = bodies[0].split_once("\n\n").expect("block after the body");
    assert_eq!(head, "do X");
    let lines: Vec<&str> = digest.lines().collect();
    assert_eq!(
        lines[0],
        "Also, 8 FYIs were queued for you: 6 from pij-sender, 2 from pij-other. The newest 3:"
    );
    assert!(
        lines[1].starts_with("6. [from pij-other, ") && lines[1].ends_with("] note 6"),
        "{digest}"
    );
    assert!(lines[2].ends_with("] note 7"), "{digest}");
    assert!(lines[3].ends_with("] note 8"), "{digest}");
    assert_eq!(lines.len(), 5, "{digest}");
    let command = lines[4]
        .strip_prefix("All 8 were delivered; read them in full with: ")
        .expect("the block says how to read them all");
    assert_eq!(pending(addr).await, 0, "every FYI is marked delivered");

    // The command it names reads all eight, in full.
    let args: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(
        &args[..4],
        ["pij-rs", "fyi-read", "--seat", RECIPIENT],
        "{command}"
    );
    assert_eq!(args[4], "--claimed-at", "{command}");
    let (status, read) = post(
        addr,
        "/v1/fyi/read",
        json!({"seat": RECIPIENT, "claimed_at_ms": args[5].parse::<u64>().expect("ms")}),
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["data"]["count"], 8);
    let full = read["data"]["block"].as_str().expect("block");
    assert!(
        full.starts_with("8 FYIs were delivered to you:\n1. [from pij-sender, "),
        "{full}"
    );
    for n in 1..=8 {
        assert_eq!(
            full.matches(&format!("] note {n}\n")).count()
                + usize::from(full.ends_with(&format!("] note {n}"))),
            1,
            "{full}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn a_hook_claim_of_a_large_pile_is_a_digest_too() {
    let store = FreshStore::new();
    let config = config(&store);
    let (addr, server) = serve(&config, true).await;
    for n in 1..=6 {
        let (status, _) = send(
            addr,
            "pij-sender",
            &format!("note {n}"),
            &format!("fyi-{n}"),
            true,
        )
        .await;
        assert_eq!(status, 200);
    }
    let (status, reply) = post(
        addr,
        "/v1/fyi/claim",
        json!({"pane": PANE, "via": "hook:claude"}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["count"], 6);
    let block = reply["data"]["block"].as_str().expect("block");
    assert!(
        block
            .starts_with("Also, 6 FYIs were queued for you: 6 from pij-sender. The newest 3:\n4. "),
        "{block}"
    );
    assert!(block.contains("\nAll 6 were delivered; read them in full with: pij-rs fyi-read --seat pij-fyi-recipient --claimed-at "), "{block}");
    server.abort();
}
