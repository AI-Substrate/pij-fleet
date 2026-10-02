//! The shim-originated endpoints (plan 119), against a real daemon over HTTP.
//!
//! These exist because the defect this unit closes was invisible to both suites.
//! `pij-literary-bonobo` registered cleanly into rs and then could not speak, and
//! nothing in either generation's tests noticed, because each half was proven
//! against itself: the shim posts `{argv, caller}`, rs's `/v1/send` wants a
//! caller-supplied `from`, and no test ever put the two on the same wire.
//!
//! So every test here drives the REAL routes with the shim's REAL body shape.

use std::net::SocketAddr;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, Harness, ProcIdentity, SeatDescriptor};
use pij_core::ports::LivenessPort;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;

const KEY: &str = "test-local-key";
const PANE: &str = "%119";
const SENDER: &str = "pij-shim-sender";
const RECIPIENT: &str = "pij-shim-recipient";
/// The recipient owns a pane too, so a READ can derive its reader the same way
/// a send derives its sender. One pane cannot be both ends of a send — that is
/// self-addressed and refused.
const RECIPIENT_PANE: &str = "%120";

async fn daemon() -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
    let mut sender = SeatDescriptor::new(SENDER, Harness::Claude, "/abs/tree");
    sender.pane = Some(PANE.to_string());
    let mut recipient = SeatDescriptor::new(RECIPIENT, Harness::Omp, "/abs/tree");
    recipient.pane = Some(RECIPIENT_PANE.to_string());
    daemon_with_seats(vec![sender, recipient]).await
}

async fn daemon_with_seats(
    seats: Vec<SeatDescriptor>,
) -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            liveness: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals-shim"),
    )
    .await
    .expect("services");
    // No push receiver exists in this HTTP/pull fixture. The default reachable
    // fake would consume the message through a fabricated socket delivery.
    services.transport = std::sync::Arc::new(pij_testkit::fakes::FakeTransport::unreachable());
    services.delivery = std::sync::Arc::new(
        pij_daemon::delivery::DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .expect("shared SQL pull sender"),
    );
    for seat in seats {
        services.registry.put(seat).await.expect("seed seat");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = router_with_config(
        services,
        HttpConfig {
            local_key: KEY.to_string(),
            peer_keys: Vec::new(),
            machine_alias: "workstation".to_string(),
        },
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, server, store)
}

async fn post(addr: SocketAddr, path: &str, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .expect("post")
}

async fn get(addr: SocketAddr, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .send()
        .await
        .expect("get")
}

#[tokio::test]
async fn list_here_scopes_get_and_post_to_the_callers_canonical_folder() {
    let root = pij_testkit::fresh_dir("pij-list-here");
    let folder = root.canonicalize().expect("canonical folder");
    let folder = folder.to_str().expect("UTF-8 folder");
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("nested folder");
    let spelling = nested.join("..").display().to_string();
    let (addr, server, _store) = daemon_with_seats(vec![
        SeatDescriptor::new("pij-here", Harness::Omp, folder),
        SeatDescriptor::new("pij-elsewhere", Harness::Omp, "/elsewhere"),
    ])
    .await;
    let client = reqwest::Client::new();
    let get = client
        .get(format!("http://{addr}/v1/seats"))
        .bearer_auth(KEY)
        .query(&[("here", spelling.as_str())])
        .send()
        .await
        .expect("GET list here");
    let post = post(
        addr,
        "/v1/seats",
        serde_json::json!({
            "argv": ["list", "--here"], "caller": {"cwd": spelling}
        }),
    )
    .await;
    let post_query = client
        .post(format!("http://{addr}/v1/seats"))
        .bearer_auth(KEY)
        .query(&[("here", spelling.as_str())])
        .json(&serde_json::json!({"argv":["list"]}))
        .send()
        .await
        .expect("POST query scope");
    for response in [get, post, post_query] {
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("list envelope");
        let ids: Vec<_> = payload["data"]["seats"]
            .as_array()
            .expect("seats")
            .iter()
            .map(|seat| seat["id"].as_str().expect("seat id"))
            .collect();
        assert_eq!(ids, ["pij-here"]);
    }
    let excluded: serde_json::Value = client
        .get(format!("http://{addr}/v1/seats"))
        .bearer_auth(KEY)
        .query(&[("here", folder), ("folder", "/elsewhere")])
        .send()
        .await
        .expect("intersect filters")
        .json()
        .await
        .expect("envelope");
    assert_eq!(excluded["data"]["seats"], serde_json::json!([]));
    server.abort();
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[tokio::test]
async fn list_here_invalid_scopes_are_decodable_argument_errors() {
    let (addr, server, _store) = daemon().await;
    for path in ["/v1/seats?here=relative", "/v1/seats?here=true"] {
        let response = get(addr, path).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let payload: serde_json::Value = response.json().await.expect("GET refusal");
        assert_eq!(payload["details"]["code"], "E-RS-ARG");
    }
    for (argv, cwd) in [
        (serde_json::json!(["list", "--here=/abs/tree"]), "/abs/tree"),
        (
            serde_json::json!(["list", "--here", "/abs/tree"]),
            "/abs/tree",
        ),
        (serde_json::json!(["list", "--here"]), "relative"),
    ] {
        let response = post(
            addr,
            "/v1/seats",
            serde_json::json!({
                "argv": argv, "caller": {"cwd": cwd}
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let payload: serde_json::Value = response.json().await.expect("POST refusal");
        assert_eq!(payload["details"]["code"], "E-RS-ARG");
    }
    let missing = post(
        addr,
        "/v1/seats",
        serde_json::json!({"argv":["list","--here"]}),
    )
    .await;
    assert_eq!(missing.status(), reqwest::StatusCode::BAD_REQUEST);
    let missing: serde_json::Value = missing.json().await.expect("missing cwd refusal");
    assert_eq!(missing["details"]["code"], "E-RS-ARG");
    server.abort();
}

/// THE CLAIM THE UNIT EXISTS FOR: a seat that registered into rs can speak,
/// with the sender DERIVED from its pane and no session id anywhere.
///
/// This is `pij-literary-bonobo`'s exact failure, at the wire. Before the route
/// existed it answered 404 with an empty body — a behavioural red, and the same
/// shape the shim reads as route-absence.
#[tokio::test]
async fn a_pane_only_caller_can_send_with_no_session_id_anywhere() {
    let (addr, server, _store) = daemon().await;

    let response = post(
        addr,
        "/v1/shim/send",
        serde_json::json!({
            "argv": ["send", RECIPIENT, "hello from a derived sender"],
            "caller": { "tmuxPane": PANE },
        }),
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "the sender must be derived from the pane, as report already is"
    );
    let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
    let receipt = payload.data.expect("receipt");
    assert!(
        receipt.get("msg_id").is_some(),
        "the receipt must name the message: {receipt}"
    );

    server.abort();
}

/// AN UNKNOWN FIELD IS REFUSED, NOT DROPPED.
///
/// This is the whole reason the endpoint is distinct rather than additive.
/// `/v1/send`'s struct TOLERATES unknown fields — measured live on 2026-09-01,
/// where a POST carrying `caller` failed on `from` and never on `caller`. So an
/// older daemon handed the additive shape accepts an asserted `from` and
/// silently discards the caller evidence meant to constrain it. Overloading
/// fails quietly; this fails loudly, and that difference is the mechanism.
#[tokio::test]
async fn a_field_this_daemon_does_not_understand_is_refused_rather_than_ignored() {
    let (addr, server, _store) = daemon().await;

    let response = post(
        addr,
        "/v1/shim/send",
        serde_json::json!({
            "argv": ["send", RECIPIENT, "hi"],
            "caller": { "tmuxPane": PANE },
            "evidence_from_a_newer_shim": "must not be dropped",
        }),
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "a newer shim's field must fail LOUD; silently dropping it is how caller \
         evidence stops constraining an asserted identity"
    );
    let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
    assert!(
        payload
            .meta
            .as_deref()
            .is_some_and(|meta| meta.contains("evidence_from_a_newer_shim")),
        "the refusal must NAME the field, or the caller cannot fix it: {:?}",
        payload.meta
    );

    server.abort();
}

/// An asserted id that CONTRADICTS the observable pane refuses BOTH — leniency
/// about absent evidence is not leniency about contradicted evidence.
#[tokio::test]
async fn an_asserted_sender_that_contradicts_the_pane_is_refused() {
    let (addr, server, _store) = daemon().await;

    let response = post(
        addr,
        "/v1/shim/send",
        serde_json::json!({
            "argv": ["send", RECIPIENT, "should not be sent as either seat"],
            "caller": { "tmuxPane": PANE, "PIJ_SESSION_ID": RECIPIENT },
        }),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
    assert!(
        payload
            .meta
            .as_deref()
            .is_some_and(|meta| meta.contains("never outranks an observable one")),
        "{:?}",
        payload.meta
    );

    server.abort();
}

/// Registration is native HTTP-only; argv inbox requests may not create seats.
#[tokio::test]
async fn inbox_register_is_refused_by_name_with_its_reason() {
    let (addr, server, _store) = daemon().await;

    let response = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({ "argv": ["inbox", "register"], "caller": { "tmuxPane": PANE } }),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
    let meta = payload.meta.unwrap_or_default();
    assert!(meta.contains("/v1/register"), "{meta}");
    assert!(meta.contains("not the shim inbox route"), "{meta}");

    server.abort();
}

/// Shapes rs cannot honour refuse BY NAME rather than being half-served.
///
/// Every one of these had an available near-fit that would have compiled — drop
/// the attachment, send the broadcast to the first recipient only — and a
/// near-fit that type-checks is the worst outcome available, because the caller
/// cannot see it.
#[tokio::test]
async fn shapes_rs_cannot_serve_refuse_by_name() {
    let (addr, server, _store) = daemon().await;

    for (argv, expect) in [
        (
            serde_json::json!(["send", "--to", "a", "--to", "b", "text"]),
            "broadcast",
        ),
        (
            serde_json::json!(["send", RECIPIENT, "--file", "/tmp/x"]),
            "attachment model",
        ),
        (
            serde_json::json!(["send", RECIPIENT, "text", "--wait"]),
            "blocks the sender",
        ),
    ] {
        let response = post(
            addr,
            "/v1/shim/send",
            serde_json::json!({ "argv": argv, "caller": { "tmuxPane": PANE } }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{argv} must refuse"
        );
        let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
        let meta = payload.meta.unwrap_or_default();
        assert!(meta.contains(expect), "{argv}: {meta}");
    }

    server.abort();
}

/// `--body-file` bytes travel in their own field and never through argv.
///
/// The parser must never see the body: on the legacy path a body re-appended to
/// argv and re-parsed turned a body beginning `--` into a FLAG, and a valued
/// flag swallowed a whole file. So the body here BEGINS with `--`, which is the
/// case that would break if it were ever lexed.
#[tokio::test]
async fn a_body_file_body_is_never_lexed_as_a_flag() {
    let (addr, server, _store) = daemon().await;

    let body = "--wait this is a body that begins with a flag\nand has two lines";
    let response = post(
        addr,
        "/v1/shim/send",
        serde_json::json!({
            "argv": ["send", RECIPIENT, "--body-file", "/tmp/whatever"],
            "caller": { "tmuxPane": PANE },
            "body_literal": body,
        }),
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "a body beginning with `--` must not be parsed as a flag"
    );

    server.abort();
}

/// Both new routes are REACHABLE at the exact paths the shim's table names.
///
/// Plan 114 shipped route rows pointing at `/v1/report/<leaf>` paths rs does not
/// serve: every call 404'd, classified as not-implemented, fell back to legacy,
/// and looked exactly like a verb a later wave had not landed — every suite
/// green. The path is a cross-runtime literal, so it gets a test that fails when
/// either side moves.
#[tokio::test]
async fn the_shim_paths_the_route_table_names_are_served() {
    let (addr, server, _store) = daemon().await;

    for path in ["/v1/shim/send", "/v1/shim/inbox"] {
        // A body this handler will refuse — the point is that it REFUSES rather
        // than 404s, because a 404 with no envelope is what the shim reads as
        // route absence and silently answers from the other store.
        let response = post(addr, path, serde_json::json!({ "argv": [] })).await;
        assert_ne!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "{path} must exist"
        );
        let payload: Envelope<serde_json::Value> = response.json().await.expect("envelope");
        assert!(!payload.ok, "{path} should have refused this body");
    }

    server.abort();
}

#[tokio::test]
async fn shim_sessions_emits_the_frozen_legacy_names_and_resolves_git_common_dir() {
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let root = pij_testkit::fresh_dir("pij-shim-sessions");
    let _cleanup = Cleanup(root.clone());
    let worktree = root.join("worktree");
    let common = root.join("common.git");
    let git_dir = common.join("worktrees").join("seat");
    std::fs::create_dir_all(&worktree).expect("create worktree");
    std::fs::create_dir_all(&git_dir).expect("create git admin dir");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .expect("write linked-worktree git file");
    std::fs::write(git_dir.join("commondir"), "../..\n").expect("write common-dir pointer");

    let mut seat = SeatDescriptor::new(
        "pij-rs-session",
        Harness::Omp,
        worktree.display().to_string(),
    );
    seat.harness_session = Some("omp-native".to_string());
    seat.parent = Some("pij-parent".into());
    seat.model = Some("provider/model".to_string());
    let mut tombstoned = SeatDescriptor::new(
        "pij-rs-tombstoned",
        Harness::Omp,
        worktree.display().to_string(),
    );
    tombstoned.harness_session = Some("dead-native".to_string());
    tombstoned.tombstoned_at = Some(1);
    let (addr, server, _store) = daemon_with_seats(vec![seat, tombstoned]).await;
    let expected_common = std::fs::canonicalize(&common).expect("canonical common dir");

    let response = get(addr, "/v1/shim/sessions").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let payload: Envelope<serde_json::Value> = response.json().await.expect("sessions envelope");
    assert_eq!(
        payload.data.expect("sessions data")["rows"],
        serde_json::json!([{
            "pijId": "pij-rs-session",
            "harness": "omp",
            "harnessSessionId": "omp-native",
            "gitCommonDir": expected_common.display().to_string(),
            "lifecycle": null,
            "boundModel": "provider/model",
            "spawnedBy": "pij-parent",
            "transcriptPath": null,
            "generation": "rs"
        }])
    );

    server.abort();
}

/// F4 — A ROUTED READ MUST FINISH. Claim, decode, acknowledge, and the next
/// read is empty.
///
/// `InboxClaim` is a two-step contract: the claim moves the job to `running` and
/// records nothing; the CLIENT acknowledges only once it HAS the message, so a
/// client that dies mid-read gets a duplicate rather than a silent loss. The
/// routed read claimed and never acked — leaving the job running, recording no
/// `ReaderRead`, and letting the same message be claimed again after the lease
/// expired, while rendering "claimed" perfectly honestly. Honest and incomplete.
///
/// The third read is what makes this a test of COMPLETION rather than of
/// plumbing: without the ack the message is still there to be claimed again.
#[tokio::test]
async fn a_routed_read_acknowledges_and_the_next_read_is_empty() {
    let (addr, server, _store) = daemon().await;

    // Give the reader something to read. The recipient owns the pane, so the
    // reader is DERIVED on both halves of the read.
    let sent = post(
        addr,
        "/v1/shim/send",
        serde_json::json!({
            "argv": ["send", RECIPIENT, "a message to claim"],
            "caller": { "tmuxPane": PANE },
        }),
    )
    .await;
    assert_eq!(sent.status(), reqwest::StatusCode::OK);

    let claimed: Envelope<Vec<serde_json::Value>> = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({ "argv": ["inbox"], "caller": { "tmuxPane": RECIPIENT_PANE } }),
    )
    .await
    .json()
    .await
    .expect("claim envelope");
    let claims = claimed.data.expect("claims");
    assert_eq!(claims.len(), 1, "one claim: {claims:?}");
    let job_id = claims[0]
        .get("job_id")
        .expect("the claim must name its job");

    let acked = post(
        addr,
        "/v1/shim/inbox/ack",
        serde_json::json!({ "caller": { "tmuxPane": RECIPIENT_PANE }, "job_id": job_id }),
    )
    .await;
    assert_eq!(
        acked.status(),
        reqwest::StatusCode::OK,
        "the read must be completable through a path that DERIVES its reader"
    );

    let again: Envelope<Vec<serde_json::Value>> = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({ "argv": ["inbox"], "caller": { "tmuxPane": RECIPIENT_PANE } }),
    )
    .await
    .json()
    .await
    .expect("second claim envelope");
    assert_eq!(
        again.data.expect("claims"),
        Vec::<serde_json::Value>::new(),
        "an acknowledged message must not be claimable again"
    );

    server.abort();
}

/// The acknowledgement derives its reader too, and refuses a caller it cannot
/// resolve — so an unresolvable caller cannot retire a message.
#[tokio::test]
async fn an_unresolvable_caller_cannot_acknowledge() {
    let (addr, server, _store) = daemon().await;

    let response = post(
        addr,
        "/v1/shim/inbox/ack",
        serde_json::json!({ "caller": { "tmuxPane": "%no-such-pane" }, "job_id": 1 }),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    // And the ack body is `deny_unknown_fields` for the same reason the send
    // body is: a field this daemon does not understand must never be dropped.
    let unknown = post(
        addr,
        "/v1/shim/inbox/ack",
        serde_json::json!({ "caller": { "tmuxPane": PANE }, "job_id": 1, "extra": true }),
    )
    .await;
    assert_eq!(unknown.status(), reqwest::StatusCode::BAD_REQUEST);

    server.abort();
}

async fn pull_seat(harness: Harness) -> (SeatDescriptor, serde_json::Value) {
    let pid = std::process::id();
    let proc_start = pij_harnesses::proc::ProcLiveness::new()
        .proc_start(pid)
        .await
        .unwrap()
        .expect("test host is live");
    let mut seat = SeatDescriptor::new(RECIPIENT, harness, "/abs/tree");
    seat.proc = Some(ProcIdentity { pid, proc_start });
    seat.harness_session = Some("native-pull-session".into());
    let mut caller = serde_json::json!({
        "PIJ_SESSION_ID": RECIPIENT,
        "pid": pid,
        "procStart": proc_start,
    });
    let key = match harness {
        Harness::Claude => "CLAUDE_CODE_SESSION_ID",
        Harness::Copilot => "COPILOT_AGENT_SESSION_ID",
        Harness::Codex => "CODEX_THREAD_ID",
        _ => unreachable!("external pull fixture"),
    };
    caller[key] = serde_json::json!("native-pull-session");
    (seat, caller)
}

#[tokio::test]
async fn external_shim_pull_round_trip_retains_body_reply_and_explicit_ack() {
    for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
        let (reader, caller) = pull_seat(harness).await;
        assert!(!reader.native_extension_delivery);
        let mut sender = SeatDescriptor::new(SENDER, Harness::Claude, "/abs/tree");
        sender.pane = Some(PANE.into());
        let (addr, server, _store) = daemon_with_seats(vec![sender, reader]).await;
        let text = "  exact body\nwith a trailing newline\n";
        let sent: Envelope<serde_json::Value> = post(
            addr,
            "/v1/shim/send",
            serde_json::json!({
                "argv": ["send", RECIPIENT, text], "caller": {"tmuxPane": PANE}
            }),
        )
        .await
        .json()
        .await
        .unwrap();
        let receipt = sent.data.expect("send receipt");
        assert_eq!(
            receipt["outcome"]["outcome"], "queued",
            "pull has no push receiver"
        );
        let message_id = receipt["msg_id"].clone();
        let claims: Envelope<Vec<serde_json::Value>> = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({
                "argv": ["inbox", "--wait", "1000"], "caller": caller
            }),
        )
        .await
        .json()
        .await
        .unwrap();
        let claims = claims.data.unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0]["message"]["body"], text);
        assert_eq!(claims[0]["message"]["msg_id"], message_id);
        assert_eq!(claims[0]["native_consumer"]["pid"], caller["pid"]);
        assert_eq!(
            claims[0]["native_consumer"]["proc_start"],
            caller["procStart"]
        );
        assert_eq!(
            claims[0]["native_consumer"]["native_session"],
            "native-pull-session"
        );
        let job_id = claims[0]["job_id"].clone();
        for field in ["pid", "procStart"] {
            let mut wrong = caller.clone();
            wrong[field] = serde_json::json!(wrong[field].as_u64().unwrap() + 1);
            let response = post(
                addr,
                "/v1/shim/inbox/ack",
                serde_json::json!({"caller": wrong, "job_id": job_id}),
            )
            .await;
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        }
        let mut wrong_session = caller.clone();
        for key in [
            "CLAUDE_CODE_SESSION_ID",
            "COPILOT_AGENT_SESSION_ID",
            "CODEX_THREAD_ID",
        ] {
            if wrong_session.get(key).is_some() {
                wrong_session[key] = serde_json::json!("wrong-native-session");
            }
        }
        assert_eq!(
            post(
                addr,
                "/v1/shim/inbox/ack",
                serde_json::json!({"caller": wrong_session, "job_id": job_id})
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post(
                addr,
                "/v1/shim/inbox/ack",
                serde_json::json!({"caller": caller, "job_id": job_id})
            )
            .await
            .status(),
            reqwest::StatusCode::OK
        );
        let reply = "  reply body\n";
        assert_eq!(
            post(
                addr,
                "/v1/shim/send",
                serde_json::json!({
                    "argv": ["send", SENDER, reply, "--in-reply-to", message_id], "caller": caller
                })
            )
            .await
            .status(),
            reqwest::StatusCode::OK
        );
        let replies: Envelope<Vec<serde_json::Value>> = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({
                "argv": ["inbox"], "caller": {"tmuxPane": PANE}
            }),
        )
        .await
        .json()
        .await
        .unwrap();
        let replies = replies.data.unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["message"]["body"], reply);
        assert_eq!(replies[0]["message"]["in_reply_to"], message_id);
        assert_eq!(
            post(
                addr,
                "/v1/shim/inbox/ack",
                serde_json::json!({"caller": {"tmuxPane": PANE}, "job_id": replies[0]["job_id"]})
            )
            .await
            .status(),
            reqwest::StatusCode::OK
        );
        assert_eq!(
            post(
                addr,
                "/v1/shim/send",
                serde_json::json!({"argv": ["send", RECIPIENT, "self"], "caller": caller})
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        let empty: Envelope<Vec<serde_json::Value>> = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({"argv": ["inbox"], "caller": caller}),
        )
        .await
        .json()
        .await
        .unwrap();
        assert!(empty.data.unwrap().is_empty());
        server.abort();
    }
}

#[tokio::test]
async fn shim_wait_timeout_is_an_empty_claims_envelope_and_rejects_invalid_durations() {
    let (reader, caller) = pull_seat(Harness::Copilot).await;
    let (addr, server, _store) = daemon_with_seats(vec![reader]).await;
    for argv in [
        serde_json::json!(["inbox", "--wait", "10", "--json"]),
        serde_json::json!(["inbox", "check", "--wait=10"]),
    ] {
        let response = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({"argv": argv, "caller": caller}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let page: Envelope<Vec<serde_json::Value>> = response.json().await.unwrap();
        assert!(page.data.unwrap().is_empty());
    }
    for args in [
        serde_json::json!(["--wait", "0"]),
        serde_json::json!(["--wait", "-1"]),
        serde_json::json!(["--wait", "1.5"]),
        serde_json::json!(["--wait="]),
        serde_json::json!(["--wait", "abc"]),
        serde_json::json!(["--wait", "1", "--wait"]),
    ] {
        let mut argv = vec![serde_json::json!("inbox")];
        argv.extend(args.as_array().unwrap().iter().cloned());
        assert_eq!(
            post(
                addr,
                "/v1/shim/inbox",
                serde_json::json!({"argv": argv, "caller": caller})
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
    }
    server.abort();
}

#[tokio::test]
async fn shim_infinite_wait_receives_later_work_and_panels_cannot_wait() {
    let (reader, caller) = pull_seat(Harness::Codex).await;
    let mut sender = SeatDescriptor::new(SENDER, Harness::Claude, "/abs/tree");
    sender.pane = Some(PANE.into());
    let (addr, server, _store) = daemon_with_seats(vec![sender, reader]).await;
    let waiting = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({"argv": ["inbox", "--wait", "--json"], "caller": caller}),
    );
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );
    assert_eq!(
        post(
            addr,
            "/v1/shim/send",
            serde_json::json!({"argv": ["send", RECIPIENT, "later"], "caller": {"tmuxPane": PANE}})
        )
        .await
        .status(),
        reqwest::StatusCode::OK
    );
    let response = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("event wakes pending request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let claims: Envelope<Vec<serde_json::Value>> = response.json().await.unwrap();
    assert_eq!(claims.data.unwrap()[0]["message"]["body"], "later");
    let refused = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({"argv": ["inbox", "--wait"], "caller": {"tmuxPane": PANE}}),
    )
    .await;
    assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
    let refusal: Envelope<serde_json::Value> = refused.json().await.unwrap();
    assert!(refusal.meta.unwrap().contains("pushed seats cannot wait"));
    server.abort();
}

#[tokio::test]
async fn shim_pull_refuses_recycled_host_before_claim_or_ack() {
    let (mut reader, mut caller) = pull_seat(Harness::Copilot).await;
    reader.proc.as_mut().unwrap().proc_start += 1;
    caller["procStart"] = serde_json::json!(reader.proc.unwrap().proc_start);
    let (addr, server, _store) = daemon_with_seats(vec![reader]).await;
    for (path, body) in [
        (
            "/v1/shim/inbox",
            serde_json::json!({"argv": ["inbox"], "caller": caller}),
        ),
        (
            "/v1/shim/inbox/ack",
            serde_json::json!({"job_id": 1, "caller": caller}),
        ),
    ] {
        let response = post(addr, path, body).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let refusal: Envelope<serde_json::Value> = response.json().await.unwrap();
        assert!(refusal.meta.unwrap().contains("no longer live"));
    }
    server.abort();
}

#[tokio::test]
async fn existing_paneless_non_copilot_callers_keep_bare_asserted_id_semantics() {
    for harness in [Harness::Claude, Harness::Codex] {
        let (reader, _) = pull_seat(harness).await;
        let mut sender = SeatDescriptor::new(SENDER, Harness::Claude, "/abs/tree");
        sender.pane = Some(PANE.into());
        let (addr, server, _store) = daemon_with_seats(vec![sender, reader]).await;
        // Old shims supplied their transient PID only, without native evidence.
        let caller = serde_json::json!({"PIJ_SESSION_ID": RECIPIENT, "pid": 1});
        assert_eq!(
            post(
                addr,
                "/v1/shim/send",
                serde_json::json!({"argv": ["send", SENDER, "old sender"], "caller": caller})
            )
            .await
            .status(),
            reqwest::StatusCode::OK
        );
        assert_eq!(post(addr, "/v1/shim/send", serde_json::json!({"argv": ["send", RECIPIENT, "old reader"], "caller": {"tmuxPane": PANE}})).await.status(), reqwest::StatusCode::OK);
        let claims: Envelope<Vec<serde_json::Value>> = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({"argv": ["inbox"], "caller": caller}),
        )
        .await
        .json()
        .await
        .unwrap();
        let claims = claims.data.unwrap();
        assert_eq!(claims[0]["message"]["body"], "old reader");
        assert!(claims[0].get("native_consumer").is_none());
        assert_eq!(
            post(
                addr,
                "/v1/shim/inbox/ack",
                serde_json::json!({"caller": caller, "job_id": claims[0]["job_id"]})
            )
            .await
            .status(),
            reqwest::StatusCode::OK
        );
        let empty: Envelope<Vec<serde_json::Value>> = post(
            addr,
            "/v1/shim/inbox",
            serde_json::json!({"argv": ["inbox"], "caller": caller}),
        )
        .await
        .json()
        .await
        .unwrap();
        assert!(empty.data.unwrap().is_empty());
        server.abort();
    }
}

#[tokio::test]
async fn reply_option_validates_once_and_does_not_lex_literal_body() {
    let (addr, server, _store) = daemon().await;
    for args in [
        serde_json::json!(["send", RECIPIENT, "body", "--in-reply-to"]),
        serde_json::json!(["send", RECIPIENT, "body", "--in-reply-to="]),
        serde_json::json!([
            "send",
            RECIPIENT,
            "body",
            "--in-reply-to",
            "a",
            "--in-reply-to",
            "b"
        ]),
    ] {
        assert_eq!(
            post(
                addr,
                "/v1/shim/send",
                serde_json::json!({"argv": args, "caller": {"tmuxPane": PANE}})
            )
            .await
            .status(),
            reqwest::StatusCode::BAD_REQUEST
        );
    }
    let body = "--in-reply-to is literal body text\n";
    assert_eq!(
        post(
            addr,
            "/v1/shim/send",
            serde_json::json!({
                "argv": ["send", RECIPIENT, "--body-file", "-", "--in-reply-to=original-id"],
                "body_literal": body,
                "caller": {"tmuxPane": PANE}
            })
        )
        .await
        .status(),
        reqwest::StatusCode::OK
    );
    let claims: Envelope<Vec<serde_json::Value>> = post(
        addr,
        "/v1/shim/inbox",
        serde_json::json!({"argv": ["inbox"], "caller": {"tmuxPane": RECIPIENT_PANE}}),
    )
    .await
    .json()
    .await
    .unwrap();
    let claims = claims.data.unwrap();
    assert_eq!(claims[0]["message"]["body"], body);
    assert_eq!(claims[0]["message"]["in_reply_to"], "original-id");
    server.abort();
}
