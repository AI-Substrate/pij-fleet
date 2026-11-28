//! `pij send-file` end to end over HTTP: one daemon, and two paired ones.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::PeerDefinition;
use pij_core::files::{E_CORRUPT, E_NO_SUCH_FILE, E_TOO_LARGE, MAX_BYTES};
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::ports::Registry as _;
use pij_testkit::fakes::{FakeQueue, FakeRegistry, FakeSpine};
use serde_json::{Value, json};

use crate::Services;
use crate::federation::{FederationService, WorkerStep};
use crate::http::tests::{config, federation_policy, spawn, test_services};
use crate::http::{AuthRing, HttpConfig, router_with_config, router_with_federation};

async fn seats(ids: &[&str]) -> Arc<FakeRegistry> {
    let registry = Arc::new(FakeRegistry::new());
    for (n, id) in ids.iter().enumerate() {
        let mut seat = SeatDescriptor::new(*id, Harness::Omp, "/abs/tree");
        seat.proc = Some(ProcIdentity {
            pid: 4_000 + n as u32,
            proc_start: 20_261_128_000_000,
        });
        registry.put(seat).await.expect("seat");
    }
    registry
}

async fn services(ids: &[&str]) -> Services {
    test_services(
        seats(ids).await,
        Arc::new(FakeQueue::new(1_024).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await
}

async fn call(
    addr: std::net::SocketAddr,
    key: &str,
    seat: &str,
    cwd: &Path,
    argv: &[&str],
) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/files"))
        .bearer_auth(key)
        .json(&json!({"argv": argv, "caller": {"PIJ_SESSION_ID": seat, "cwd": cwd}}))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("envelope")
}

fn code_of(answer: &Value) -> String {
    answer["data"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no code: {answer}"))
        .to_string()
}

fn refusal_code(answer: &Value) -> &str {
    answer["details"]["code"].as_str().unwrap_or_default()
}

fn stored(services: &Services) -> Vec<PathBuf> {
    std::fs::read_dir(services.transfers.root())
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

async fn fyis(services: &Services, seat: &str) -> u64 {
    for _ in 0..50 {
        let count = services
            .delivery
            .pending_fyi_count(&SeatId::from(seat))
            .await
            .expect("fyi count");
        if count > 0 {
            return count;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    0
}

#[tokio::test]
async fn a_file_goes_to_its_recipient_only_and_is_deleted_once_saved() {
    let services = services(&["pij-alice", "pij-bob", "pij-carol"]).await;
    let kept = services.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let dir = pij_testkit::fresh_dir("pij-files-local");
    std::fs::write(dir.join("plan.txt"), b"the plan").expect("source");

    let sent = call(
        addr,
        "key",
        "pij-alice",
        &dir,
        &["send-file", "pij-bob", "plan.txt", "--note", "for you"],
    )
    .await;
    assert_eq!(sent["ok"], true, "{sent}");
    let code = code_of(&sent);
    assert_eq!(sent["data"]["size"], 8);

    let listed = call(addr, "key", "pij-bob", &dir, &["file", "list"]).await;
    assert_eq!(
        listed["data"]["files"][0]["code"],
        code.as_str(),
        "{listed}"
    );
    assert_eq!(listed["data"]["files"][0]["from"], "pij-alice");
    let theirs = call(addr, "key", "pij-carol", &dir, &["file", "list"]).await;
    assert_eq!(theirs["data"]["files"], json!([]));

    let stolen = call(
        addr,
        "key",
        "pij-carol",
        &dir,
        &["file", "accept", &code, "--to", "mine.txt"],
    )
    .await;
    assert_eq!(refusal_code(&stolen), E_NO_SUCH_FILE, "{stolen}");
    assert!(!dir.join("mine.txt").exists());

    std::fs::write(dir.join("taken.txt"), b"already here").expect("existing");
    let clash = call(
        addr,
        "key",
        "pij-bob",
        &dir,
        &["file", "accept", &code, "--to", "taken.txt"],
    )
    .await;
    assert_eq!(refusal_code(&clash), pij_core::files::E_TARGET, "{clash}");
    assert_eq!(
        std::fs::read(dir.join("taken.txt")).expect("kept"),
        b"already here"
    );

    let saved = call(
        addr,
        "key",
        "pij-bob",
        &dir,
        &["file", "accept", &code, "--to", "got.txt"],
    )
    .await;
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(
        std::fs::read(dir.join("got.txt")).expect("saved"),
        b"the plan"
    );
    assert!(stored(&kept).is_empty(), "{:?}", stored(&kept));
    assert_eq!(fyis(&kept, "pij-alice").await, 1, "the sender is told");

    let again = call(
        addr,
        "key",
        "pij-bob",
        &dir,
        &["file", "accept", &code, "--to", "again.txt"],
    )
    .await;
    assert_eq!(refusal_code(&again), E_NO_SUCH_FILE);
    server.abort();
}

#[tokio::test]
async fn a_declined_file_is_deleted_unsaved_and_the_sender_told() {
    let services = services(&["pij-alice", "pij-bob"]).await;
    let kept = services.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let dir = pij_testkit::fresh_dir("pij-files-decline");
    std::fs::write(dir.join("x.bin"), b"\x00\x01").expect("source");
    let code = code_of(
        &call(
            addr,
            "key",
            "pij-alice",
            &dir,
            &["send-file", "pij-bob", "x.bin"],
        )
        .await,
    );
    let declined = call(addr, "key", "pij-bob", &dir, &["file", "decline", &code]).await;
    assert_eq!(declined["ok"], true, "{declined}");
    assert!(stored(&kept).is_empty());
    assert_eq!(fyis(&kept, "pij-alice").await, 1);
    server.abort();
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_with_its_size() {
    let services = services(&["pij-alice", "pij-bob"]).await;
    let kept = services.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let dir = pij_testkit::fresh_dir("pij-files-large");
    // Sparse: its size is the point, not its bytes.
    let big = std::fs::File::create(dir.join("big.bin")).expect("big");
    big.set_len(MAX_BYTES + 1).expect("size");
    let refused = call(
        addr,
        "key",
        "pij-alice",
        &dir,
        &["send-file", "pij-bob", "big.bin"],
    )
    .await;
    assert_eq!(refusal_code(&refused), E_TOO_LARGE, "{refused}");
    assert!(
        refused["meta"]
            .as_str()
            .unwrap_or_default()
            .contains("52428801 bytes"),
        "{refused}"
    );
    assert!(stored(&kept).is_empty());
    server.abort();
}

fn laptop_config() -> HttpConfig {
    HttpConfig {
        auth: AuthRing::new(
            "b-local".to_string(),
            [("desktop".to_string(), "pair-key".to_string())],
        )
        .expect("ring"),
        machine_alias: "laptop".to_string(),
    }
}

fn paired(local_key: &str, peer: &str, machine: &str) -> HttpConfig {
    HttpConfig {
        auth: AuthRing::new(
            local_key.to_string(),
            [(peer.to_string(), "pair-key".to_string())],
        )
        .expect("ring"),
        machine_alias: machine.to_string(),
    }
}

fn federation_to(
    services: &Services,
    machine: &str,
    peer: &str,
    addr: std::net::SocketAddr,
) -> Arc<FederationService> {
    Arc::new(
        FederationService::new(
            machine.to_string(),
            [PeerDefinition {
                alias: peer.to_string(),
                url: format!("http://{addr}"),
                key: "pair-key".to_string(),
            }],
            Arc::clone(&services.queue),
            Arc::clone(&services.event_bus),
            federation_policy(),
        )
        .expect("federation"),
    )
}

fn serve(listener: tokio::net::TcpListener, router: axum::Router) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("serve");
    })
}

/// Two daemons paired both ways: the file streams to the laptop and is stored
/// only there; the laptop tells the desktop's sender how it ended.
#[tokio::test]
async fn a_file_streams_to_a_paired_machine_and_the_sender_hears_back() {
    let bind = || tokio::net::TcpListener::bind("127.0.0.1:0");
    let (a_listener, b_listener) = (bind().await.expect("a"), bind().await.expect("b"));
    let (a_addr, b_addr) = (
        a_listener.local_addr().expect("a"),
        b_listener.local_addr().expect("b"),
    );
    let (desktop, laptop) = (services(&["pij-alice"]).await, services(&["pij-bob"]).await);
    let (on_desktop, on_laptop) = (desktop.clone(), laptop.clone());
    let a_federation = federation_to(&desktop, "desktop", "laptop", b_addr);
    let b_federation = federation_to(&laptop, "laptop", "desktop", a_addr);
    let a_server = serve(
        a_listener,
        router_with_federation(
            desktop,
            paired("a-local", "laptop", "desktop"),
            a_federation,
        ),
    );
    let b_server = serve(
        b_listener,
        router_with_federation(
            laptop,
            paired("b-local", "desktop", "laptop"),
            Arc::clone(&b_federation),
        ),
    );

    let dir = pij_testkit::fresh_dir("pij-files-remote");
    let bytes: Vec<u8> = (0..300_000_u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(dir.join("data.bin"), &bytes).expect("source");
    let sent = call(
        a_addr,
        "a-local",
        "pij-alice",
        &dir,
        &["send-file", "pij-bob@laptop", "data.bin"],
    )
    .await;
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(sent["data"]["to"], "pij-bob@laptop");
    let code = code_of(&sent);
    assert!(stored(&on_desktop).is_empty(), "the sender's copy is gone");

    let listed = call(b_addr, "b-local", "pij-bob", &dir, &["file", "list"]).await;
    assert_eq!(
        listed["data"]["files"][0]["from"], "pij-alice@desktop",
        "{listed}"
    );
    let saved = call(
        b_addr,
        "b-local",
        "pij-bob",
        &dir,
        &["file", "accept", &code, "--to", "got.bin"],
    )
    .await;
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(std::fs::read(dir.join("got.bin")).expect("saved"), bytes);
    assert!(stored(&on_laptop).is_empty());

    // No federation worker runs in a test: forward the outcome by hand.
    let mut forwarded = false;
    for _ in 0..50 {
        if b_federation.process_one().await.expect("forward") == WorkerStep::Forwarded {
            forwarded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(forwarded, "the laptop queued the outcome for the desktop");
    assert_eq!(
        fyis(&on_desktop, "pij-alice").await,
        1,
        "the sender is told"
    );
    a_server.abort();
    b_server.abort();
}

#[tokio::test]
async fn a_streamed_file_that_does_not_match_its_hash_is_not_kept() {
    let laptop = services(&["pij-bob"]).await;
    let kept = laptop.clone();
    let (addr, server) = spawn(router_with_config(laptop, laptop_config())).await;
    let meta = json!({
        "from": "pij-alice", "to": "pij-bob", "name": "a.txt", "size": 5,
        "sha256": "0".repeat(64),
    });
    let header = pij_core::files::hex(&serde_json::to_vec(&meta).expect("meta"));
    let post = |key: &'static str| {
        reqwest::Client::new()
            .post(format!("http://{addr}/v1/files/inbound"))
            .bearer_auth(key)
            .header("x-pij-file", header.clone())
            .body("hello")
            .send()
    };
    let answer: Value = post("pair-key")
        .await
        .expect("post")
        .json()
        .await
        .expect("json");
    assert_eq!(refusal_code(&answer), E_CORRUPT, "{answer}");
    assert!(stored(&kept).is_empty());
    let local = post("b-local").await.expect("post");
    assert_eq!(
        local.status().as_u16(),
        400,
        "only a paired machine streams a file in"
    );
    server.abort();
}
