//! Regressions for the seven review findings (2026-08-28, `pij-right-wasp`).
//!
//! One test per finding that had a testable observable, each named for the
//! failure rather than the fix — a regression test's job is to fail the way the
//! bug failed, so a future change that reintroduces it is stopped by a message
//! that describes the incident.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::error::PijError;
use pij_core::model::{Harness, Msg, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine, Transport};
use pij_harnesses::{SpawnPlanInput, build_spawn_plan};
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::FreshStore;
use pij_transport::UdsTransport;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;

fn state_dir() -> PathBuf {
    pij_testkit::fresh_dir("pij-rs-regression")
}

fn write_uds_record(dir: &Path, socket: &Path) -> u64 {
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let pid = std::process::id();
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
        .expect("observe live fixture");
    assert!(output.status.success());
    let start = String::from_utf8(output.stdout).expect("start text");
    let start = start.trim();
    std::fs::write(
        dir.join(format!("{pid}.json")),
        serde_json::json!({
            "pid": pid, "procStart": start, "pidDomain": "darwin", "tmux": "pij:@1.%108",
            "messagingSocketPath": socket,
        })
        .to_string(),
    )
    .expect("session record");
    std::fs::write(
        dir.join(format!("{pid}.{HASH}.key")),
        serde_json::json!({
            "peerToken": "observed-token", "procStart": start, "pidDomain": "darwin",
        })
        .to_string(),
    )
    .expect("peer key");
    pij_core::model::parse_process_start(start).expect("captured UTC start")
}

fn offline(bind: &str) -> Config {
    Config {
        bind_addr: bind.to_string(),
        ..Config::default()
    }
}

/// FINDING 1a — `OpenOptions::mode()` only applies to a file it CREATES, so a
/// restart into a state dir holding a 0644 `daemon.key` republished the key
/// world-readable.
#[cfg(unix)]
#[tokio::test]
async fn a_pre_existing_lax_key_file_is_replaced_not_reused() {
    use std::os::unix::fs::PermissionsExt;

    let dir = state_dir();
    let path = dir.join("daemon.key");
    std::fs::write(&path, "stale-and-world-readable").expect("plant a 0644 key");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");

    let daemon = pij_daemon::boot(&offline("127.0.0.1:0"), dir.clone())
        .await
        .expect("boot");

    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the published key must be 0600 even when a laxer file was already there"
    );
    assert_ne!(
        std::fs::read_to_string(&path).expect("read"),
        "stale-and-world-readable",
        "the old key's CONTENT must be gone too"
    );

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

/// FINDING 1b — the worst half: a second daemon wrote its key BEFORE binding, so
/// a boot that then lost the port race had already replaced the running daemon's
/// credential. A healthy daemon was bricked by starting a doomed one.
#[tokio::test]
async fn a_failed_boot_cannot_brick_the_running_daemon() {
    let dir = state_dir();
    let first = pij_daemon::boot(&offline("127.0.0.1:0"), dir.clone())
        .await
        .expect("first boot");
    let live_token = first.key.token.clone();
    let live_runtime =
        std::fs::read(dir.join(pij_daemon::DAEMON_RUNTIME_FILE)).expect("read live runtime record");

    // Same state dir, same port: the bind must fail.
    let error = pij_daemon::boot(&offline(&first.addr.to_string()), dir.clone())
        .await
        .expect_err("the second boot must fail to bind");
    assert!(
        error
            .to_string()
            .contains("another daemon may already hold it")
    );

    // The live daemon's key survived, byte for byte...
    assert_eq!(
        std::fs::read_to_string(dir.join("daemon.key")).expect("read"),
        live_token,
        "a failed boot must not touch the published key"
    );
    assert_eq!(
        std::fs::read(dir.join(pij_daemon::DAEMON_RUNTIME_FILE)).expect("read runtime record"),
        live_runtime,
        "a failed boot must not replace the running daemon's process identity"
    );

    // ...and still authenticates.
    let status = reqwest::Client::new()
        .get(format!("http://{}/health", first.addr))
        .header(reqwest::header::AUTHORIZATION, first.key.header())
        .send()
        .await
        .expect("request")
        .status();
    assert_eq!(status, reqwest::StatusCode::OK);

    // No staging litter left behind either: a stray file that looks like a
    // credential is its own hazard.
    let strays: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        // "sidecars" is a DIRECTORY the sidecar consumers own (bg state, chore
        // baseline), created at construction. This assertion is about credential
        // LITTER — a stray file that looks like a key — not about every artifact
        // a subsystem legitimately owns.
        .filter(|name| {
            name != "daemon.key" && name != pij_daemon::DAEMON_RUNTIME_FILE && name != "sidecars"
        })
        .collect();
    assert!(
        strays.is_empty(),
        "unexpected files in the state dir: {strays:?}"
    );

    first.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

/// FINDING 3 — the composition root wired two ports while `offline` was computed
/// from seven, so a config could report `offline=false` with nothing constructed.
#[tokio::test]
async fn the_composition_root_wires_every_port_it_claims() {
    let services = pij_daemon::build_services(
        &Config::default(),
        std::path::Path::new("/tmp/pij-test-pane-signals"),
    )
    .await
    .expect("all-fake services");

    // Each port is present and answers — a null wiring would panic or refuse.
    assert!(services.registry.get(&SeatId::from("nobody")).await.is_ok());
    assert!(services.spine.tail(None, Seq(0)).await.is_ok());
    assert!(services.queue.claim(&[], "w").await.is_ok());
    assert!(services.tmux.list_panes().await.is_ok());
    assert!(services.liveness.proc_start(1).await.is_ok());
    // Wave 2 replaced the single harness adapter with a registry: a composite
    // adapter's kind() had no truthful value, so every variant now has its own.
    // The assertion follows the shape rather than being deleted with it.
    for kind in [
        Harness::Claude,
        Harness::Copilot,
        Harness::Codex,
        Harness::Pi,
        Harness::Omp,
    ] {
        assert_eq!(
            services.harnesses.get(kind).kind(),
            kind,
            "the registry must return the adapter it was asked for"
        );
    }
    assert_eq!(services.transport.name(), "fake");
    assert!(services.offline);
}

/// FINDING 3b, FLIPPED FOR THE LAST TIME — the refusal is gone because there is
/// no port left that can be selected and not built. All seven are real-capable
/// as of u-uds.
///
/// The assertion flips rather than being deleted, exactly as it did for liveness
/// in wave 1: selecting `Real` must hand back something that is really the
/// socket transport, and it is distinguished BEHAVIOURALLY rather than by type
/// name, because a fake wearing the right name would pass a name check.
///
/// The behaviour that identifies it is the one u-uds MEASURED: a real
/// `UdsTransport` refuses a seat whose descriptor does not carry
/// `cross_session_inbound_accept: Some(true)`, because an authenticated
/// daemon-origin frame to such a seat is held behind a Deny/Deliver dialog,
/// reaches the model never, and returns no bytes at all. `FakeTransport::reachable`
/// says yes to that same seat.
#[tokio::test]
async fn selecting_the_real_transport_ships_closed_until_a_seat_opts_in() {
    let config = Config {
        adapters: Adapters {
            transport: AdapterChoice::Real,
            ..Adapters::default()
        },
        ..Config::default()
    };
    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-pane-signals"))
            .await
            .expect("the real transport exists now and must build");

    let mut seat = SeatDescriptor::new("pij-claude-seat", Harness::Claude, "/abs/tree");
    seat.proc = Some(ProcIdentity {
        pid: std::process::id(),
        proc_start: 1,
    });
    assert_eq!(
        seat.cross_session_inbound_accept, None,
        "unknown by default"
    );

    let msg = Msg {
        from: SeatId::from("pij-sender"),
        to: seat.id.clone(),
        body: "body".to_string(),
        msg_id: "m-1".to_string(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };
    assert!(
        !services
            .transport
            .can_deliver(&seat, &msg)
            .await
            .expect("capability is a cheap descriptor question, not an IO failure"),
        "an unknown accept precondition must be treated as CLOSED — a fake would say yes here"
    );
}

/// Wave 6 rule 1.10 — spawn argv and transport capability are one agreement.
/// Each side already had tests; only this composition-root test proves that the
/// argv-derived stamp is the exact fact the real adapter consumes.
#[tokio::test]
async fn emitted_spawn_argv_is_the_stamp_that_opens_the_real_transport() {
    use pij_core::ports::LivenessPort as _;
    let plan = |accept_inbound| {
        build_spawn_plan(SpawnPlanInput {
            // The seat id the launcher assigned: spawn emits it as
            // PIJ_SESSION_ID so `pij inbox` can resolve who it is.
            seat_id: SeatId::from("pij-stamped"),
            spawn_id: "spawn-stamped".to_string(),
            harness: Harness::Claude,
            executable: None,
            model: None,
            resolved_provider: None,
            effort: None,
            accept_inbound,
            resume: None,
        })
        .expect("Claude spawn plan")
    };
    let dir = state_dir();
    let socket = std::env::temp_dir().join(format!("pij-uds-agree-{}.sock", std::process::id()));
    let listener = UnixListener::bind(&socket).expect("bind socket witness");
    let utc_start = write_uds_record(&dir, &socket);
    let transport = UdsTransport::from_sessions_dir(dir.clone());
    let mut seat = SeatDescriptor::new("pij-claude-seat", Harness::Claude, "/abs/tree");
    seat.proc = Some(ProcIdentity {
        pid: std::process::id(),
        proc_start: pij_harnesses::proc::ProcLiveness::new()
            .proc_start(std::process::id())
            .await
            .expect("observe fixture")
            .expect("fixture process alive"),
    });
    seat.pane = Some("%108".to_string());
    let msg = Msg {
        from: SeatId::from("pij-sender"),
        to: seat.id.clone(),
        body: "joined agreement".to_string(),
        msg_id: "spawn-transport-agreement".to_string(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };

    let closed = plan(false);
    assert!(!closed.args.iter().any(|arg| arg == "--settings"));
    seat.cross_session_inbound_accept = Some(closed.cross_session_inbound_accept);
    assert!(
        !transport
            .can_deliver(&seat, &msg)
            .await
            .expect("closed capability")
    );
    assert_eq!(
        transport
            .deliver(&seat, &msg)
            .await
            .expect("closed delivery"),
        pij_core::model::DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        }
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err(),
        "argv without acceptance must produce no socket connection"
    );

    let accepted = plan(true);
    assert!(accepted.args.windows(2).any(|pair| {
        pair[0] == "--settings" && pair[1] == r#"{"crossSessionInbound":"accept"}"#
    }));
    seat.cross_session_inbound_accept = Some(accepted.cross_session_inbound_accept);
    assert!(
        transport
            .can_deliver(&seat, &msg)
            .await
            .expect("open capability")
    );
    let local_start = seat.proc.expect("fixture identity").proc_start;
    if local_start == utc_start {
        use std::io::Write as _;
        // Direct stderr bypasses libtest's successful-test output capture.
        // Only this sub-property skips: capability agreement still runs below.
        writeln!(std::io::stderr().lock(),
            "SKIP timezone-separation property: local == UTC ({local_start}); this host cannot exercise the local/UTC split. Capability agreement still runs."
        ).expect("report unexercised timezone property");
    } else {
        assert_ne!(
            local_start, utc_start,
            "this host exercises two distinct timebases"
        );
        let mut exact_identity = seat.clone();
        exact_identity.pane = None;
        assert!(
            transport
                .can_deliver(&exact_identity, &msg)
                .await
                .expect("exact local identity plus UTC record"),
            "pane fallback must not rescue an incorrect local/UTC comparison"
        );
    }
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("transport connects");
        let mut reader = BufReader::new(stream);
        let mut received = String::new();
        reader.read_line(&mut received).await.expect("auth frame");
        reader
            .read_line(&mut received)
            .await
            .expect("message frame");
        tokio::time::sleep(Duration::from_millis(200)).await;
        received
    });
    assert_eq!(
        transport.deliver(&seat, &msg).await.expect("open delivery"),
        pij_core::model::DeliveryOutcome::Delivered {
            origin: pij_core::model::DeliveryOrigin::InjectedToTransport
        }
    );
    let received = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("socket reached")
        .expect("socket witness");
    assert!(received.contains("\"token\":\"observed-token\""));
    assert!(received.contains("\"msg_id\":\"spawn-transport-agreement\""));
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_dir_all(dir);
}

/// WAVE 1 — the refusal for `liveness` is GONE because the adapter now exists,
/// and the assertion flips rather than being deleted: selecting `Real` must hand
/// back something that actually reaches the process table.
///
/// Distinguished behaviourally, not by type name: `FakeLiveness` knows only the
/// processes a test arranged, so it reports this very process as absent, while
/// the real adapter reports a start time for it. A test that checked the type
/// would pass on a fake wearing the right name.
#[tokio::test]
async fn selecting_the_real_liveness_adapter_reaches_the_process_table() {
    let config = Config {
        adapters: Adapters {
            liveness: AdapterChoice::Real,
            ..Adapters::default()
        },
        ..Config::default()
    };

    let services = match pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-pane-signals"),
    )
    .await
    {
        Ok(services) => services,
        Err(error) => panic!("the real liveness adapter must build: {error}"),
    };

    let mine = std::process::id();
    assert!(
        services
            .liveness
            .proc_start(mine)
            .await
            .expect("the real adapter must not error on our own pid")
            .is_some(),
        "a real LivenessPort must see this test's own process"
    );
    assert_eq!(
        pij_daemon::build_services(
            &Config::default(),
            std::path::Path::new("/tmp/pij-test-pane-signals"),
        )
        .await
        .expect("all-fake")
        .liveness
        .proc_start(mine)
        .await
        .expect("fake"),
        None,
        "...and the fake must NOT — which is what proves the wiring changed"
    );
}

/// FINDING 5 — `require_current_schema` existed but nothing called it, so the
/// documented "every db-touching command checks" was true only of tests. A schema
/// can change under a live pool: another binary migrating the same file.
#[tokio::test]
async fn a_schema_that_changes_under_a_live_pool_stops_the_next_operation() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let bus = Arc::new(
        pij_daemon::events::EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16)
            .expect("shared event bus"),
    );
    let registry = SqliteRegistry::new(pool.clone(), bus);

    // Healthy first, so the refusal below cannot be blamed on a broken store.
    registry
        .put(SeatDescriptor::new("pij-before", Harness::Pi, "/abs"))
        .await
        .expect("a current schema accepts writes");

    sqlx::query("PRAGMA user_version = 99")
        .execute(&pool)
        .await
        .expect("another binary migrates the same file");

    for (label, result) in [
        ("get", registry.get(&SeatId::from("pij-before")).await.err()),
        (
            "put",
            registry
                .put(SeatDescriptor::new("pij-after", Harness::Pi, "/abs"))
                .await
                .err(),
        ),
        (
            "tombstone",
            registry
                .tombstone(&SeatId::from("pij-before"), "x")
                .await
                .err(),
        ),
        (
            "spine append",
            SqliteSpine::new(pool.clone())
                .append(pij_core::model::Event {
                    seq: None,
                    v: 1,
                    at: 0,
                    kind: "report".to_string(),
                    seat: None,
                    payload: "{}".to_string(),
                })
                .await
                .err(),
        ),
    ] {
        match result {
            Some(PijError::StoreSchemaStale { found, .. }) => assert_eq!(found, 99),
            other => panic!("{label} must refuse a changed schema, got {other:?}"),
        }
    }
}

/// FINDING 6 — the event and the row were separate autocommit statements, so a
/// tombstone that refused (no such seat) had already committed a spine event
/// claiming it happened.
#[tokio::test]
async fn a_refused_tombstone_leaves_no_trace_in_the_spine() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let bus = Arc::new(
        pij_daemon::events::EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16)
            .expect("shared event bus"),
    );
    let registry = SqliteRegistry::new(pool.clone(), bus);
    let spine = SqliteSpine::new(pool.clone());

    let before = spine.tail(None, Seq(0)).await.expect("tail").len();

    let error = registry
        .tombstone(&SeatId::from("pij-never-registered"), "no such seat")
        .await
        .expect_err("tombstoning an absent seat must fail");
    assert!(matches!(error, PijError::NoRegistryEntry { .. }));

    let after = spine.tail(None, Seq(0)).await.expect("tail");
    assert_eq!(
        after.len(),
        before,
        "a refused mutation must not leave a spine event behind: {after:?}"
    );
    assert!(
        !after.iter().any(|event| event.kind == "seat.tombstone"),
        "the phantom tombstone is exactly the finding: {after:?}"
    );
}

/// FINDING 6b — the same guarantee for the success path: `put` writes the event
/// and the row as ONE fact.
#[tokio::test]
async fn a_successful_put_writes_its_event_and_its_row_together() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let bus = Arc::new(
        pij_daemon::events::EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16)
            .expect("shared event bus"),
    );
    let registry = SqliteRegistry::new(pool.clone(), bus);
    let spine = SqliteSpine::new(pool);

    let seat = SeatDescriptor::new("pij-atomic", Harness::Pi, "/abs");
    let seq = registry.put(seat.clone()).await.expect("put");

    assert!(registry.get(&seat.id).await.expect("get").is_some());
    let events = spine.tail(None, Seq(0)).await.expect("tail");
    assert!(
        events.iter().any(|event| event.kind == "seat.put"),
        "the write's event must be readable: {events:?}"
    );
    assert!(seq > Seq(0));
}

/// ROUND 2, FINDING 1 — staging was unique per PROCESS, not per ATTEMPT. Two boots
/// inside one process shared `daemon.key.staging.<pid>`, and the second one's
/// pre-delete-then-recreate meant an attempt could publish the OTHER attempt's
/// bytes — then start holding a credential nobody has.
#[test]
fn two_staged_keys_in_one_process_never_share_a_file() {
    let dir = state_dir();

    let first = pij_daemon::auth::stage_key(&dir).expect("stage first");
    let second = pij_daemon::auth::stage_key(&dir).expect("stage second");
    assert_ne!(
        first.token(),
        second.token(),
        "two attempts must generate different secrets"
    );

    // Publish the SECOND while the first is still staged: the bytes on disk must
    // be the ones its own `BootKey` reports.
    let expected = second.token().to_string();
    let published = second.publish().expect("publish");
    assert_eq!(published.token, expected);
    assert_eq!(
        std::fs::read_to_string(&published.path).expect("read"),
        expected,
        "an attempt must publish ITS OWN bytes, never another attempt's"
    );

    // Then the first, which must overwrite cleanly with its own.
    let expected_first = first.token().to_string();
    let published_first = first.publish().expect("publish");
    assert_eq!(
        std::fs::read_to_string(&published_first.path).expect("read"),
        expected_first
    );

    // No staging litter survives either attempt.
    let strays: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("staging"))
        .collect();
    assert!(strays.is_empty(), "staging files left behind: {strays:?}");

    let _ = std::fs::remove_dir_all(dir);
}

/// ROUND 2, FINDING 1b — a staged key that is never published leaves nothing
/// behind, so a failed boot cannot litter the state dir with something that looks
/// like a credential.
#[test]
fn an_unpublished_staged_key_removes_itself() {
    let dir = state_dir();
    {
        let _staged = pij_daemon::auth::stage_key(&dir).expect("stage");
        assert_eq!(
            std::fs::read_dir(&dir).expect("read dir").count(),
            1,
            "the staging file exists while the attempt is alive"
        );
    }
    assert_eq!(
        std::fs::read_dir(&dir).expect("read dir").count(),
        0,
        "dropping an unpublished attempt must remove its staging file"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// ROUND 2, FINDING 2 — the schema guard covered Registry and Spine but not
/// `SqliteQueue::live_len`, so "every public database operation checks" was true
/// of every operation I remembered. This test enumerates EVERY public database
/// operation across all three ports, so the next one added is covered or this
/// fails.
#[tokio::test]
async fn every_public_store_operation_rejects_a_stale_schema() {
    use pij_core::model::{Job, JobId, Outcome};
    use pij_core::ports::{Queue, SeatFilter};

    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let bus = Arc::new(
        pij_daemon::events::EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16)
            .expect("shared event bus"),
    );
    let registry = SqliteRegistry::new(pool.clone(), bus);
    let spine = SqliteSpine::new(pool.clone());
    let queue = pij_store::SqliteQueue::new(
        pool.clone(),
        Config::default().claim_lease_secs,
        Config::default().delivered_id_capacity,
    )
    .expect("valid default queue policy");

    sqlx::query("PRAGMA user_version = 99")
        .execute(&pool)
        .await
        .expect("another binary migrated the same file");

    let job = Job {
        kind: "deliver".to_string(),
        serial_key: "seat".to_string(),
        payload: "{}".to_string(),
        dedupe_key: "d".to_string(),
        attempt: 0,
    };
    let event = pij_core::model::Event {
        seq: None,
        v: 1,
        at: 0,
        kind: "report".to_string(),
        seat: None,
        payload: "{}".to_string(),
    };

    let outcomes: Vec<(&str, Option<PijError>)> = vec![
        ("registry.get", registry.get(&SeatId::from("x")).await.err()),
        (
            "registry.put",
            registry
                .put(SeatDescriptor::new("x", Harness::Pi, "/abs"))
                .await
                .err(),
        ),
        (
            "registry.list",
            registry.list(SeatFilter::default()).await.err(),
        ),
        (
            "registry.tombstone",
            registry.tombstone(&SeatId::from("x"), "r").await.err(),
        ),
        ("spine.append", spine.append(event).await.err()),
        ("spine.tail", spine.tail(None, Seq(0)).await.err()),
        ("queue.enqueue", queue.enqueue(job).await.err()),
        (
            "queue.claim",
            queue.claim(&["deliver".to_string()], "w").await.err(),
        ),
        ("queue.ack", queue.ack(JobId(1), Outcome::Done).await.err()),
        ("queue.live_len", queue.live_len().await.err()),
    ];

    for (name, error) in outcomes {
        match error {
            Some(PijError::StoreSchemaStale { found, .. }) => assert_eq!(found, 99, "{name}"),
            other => panic!("{name} must refuse a stale schema, got {other:?}"),
        }
    }
}

/// ROUND 3, FINDING 2 — `StagedKey` was `Clone`, so two values owned one staging
/// path and either one's `Drop` deleted it. Dropping a clone made the original's
/// `publish` fail with "No such file or directory". A guard that can be
/// duplicated is not a guard, and the compiler is the right place to say so.
///
/// This test cannot construct the bug any more — that is the point. It asserts
/// the property the missing `Clone` protects: a staged key survives until ITS
/// owner publishes or drops it, and unrelated staging activity cannot remove it.
#[test]
fn a_staged_key_survives_other_attempts_coming_and_going() {
    let dir = state_dir();

    let mine = pij_daemon::auth::stage_key(&dir).expect("stage mine");
    let expected = mine.token().to_string();

    // Several other attempts start and die while mine is still staged.
    for _ in 0..3 {
        let other = pij_daemon::auth::stage_key(&dir).expect("stage other");
        assert_ne!(other.token(), mine.token());
        drop(other);
    }

    let published = mine.publish().expect("my staged key must still be there");
    assert_eq!(
        std::fs::read_to_string(&published.path).expect("read"),
        expected,
        "another attempt's lifecycle must not touch my bytes"
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// ROUND 4 — the compile-time half of the previous finding.
///
/// The property test above proves attempts do not share paths. It does NOT prove
/// what the missing `Clone` protects: the reviewer re-added `#[derive(Clone)]`
/// and the test still passed, so the defect could return with its own regression
/// staying green. A test that cannot fail for the reason the bug would is not
/// covering the bug.
///
/// This detects `Clone` at COMPILE time, without a proc-macro dependency, via
/// autoref specialisation: the inherent method on `Probe<T>` requires
/// `T: Clone` and wins method resolution when it applies; otherwise resolution
/// falls back through the reference to the trait impl. So `is_clone()` answers
/// "does this type implement Clone" as a constant the compiler chose.
#[test]
fn staged_key_must_not_be_clone_or_two_owners_share_one_drop() {
    use std::marker::PhantomData;

    struct Probe<T>(PhantomData<T>);

    impl<T: Clone> Probe<T> {
        fn is_clone(&self) -> bool {
            true
        }
    }

    trait NotClone {
        fn is_clone(&self) -> bool {
            false
        }
    }

    impl<T> NotClone for &Probe<T> {}

    // Sanity first: the probe must be able to SEE a Clone type, or the assertion
    // below would pass for the wrong reason (a probe that always answers false
    // is a test that always passes).
    assert!(
        Probe::<String>(PhantomData).is_clone(),
        "the probe itself is broken — it cannot detect a type that IS Clone"
    );
    assert!(
        Probe::<pij_daemon::BootKey>(PhantomData).is_clone(),
        "BootKey is deliberately Clone; if that changed, this probe needs revisiting"
    );

    // The assertion that matters. The `&` is LOAD-BEARING here and nowhere else:
    // for a non-Clone type there is no inherent method to find, so resolution
    // must reach the trait impl on `&Probe<T>`. clippy flags the borrow on the
    // two probes above — correctly, since the inherent method applies there — and
    // stays silent on this one, which is the difference the trick turns on.
    assert!(
        !(&Probe::<pij_daemon::auth::StagedKey>(PhantomData)).is_clone(),
        "StagedKey must NOT be Clone: its Drop deletes the staged file, so a second \
         owner deletes the first owner's key and `publish` fails with 'No such file \
         or directory'. A guard that can be duplicated is not a guard."
    );
}
