//! Plan 174 (pij-fleet#55): a double seat, one conversation running in two
//! panes, is refused with `E-RS-CONVERSATION-LIVE` and announced to the live
//! seat once.
//!
//! The incident (2026-10-15): pij revived `pij-different-smelt` in pane %5 and
//! the same Claude conversation was resumed by hand in %7. Every verb from %7
//! failed with "not registered", and %5 was never told.
//!
//! Fixtures only. The pids sit far above any real pid, so the real `ps` walk in
//! adopt finds nothing and the liveness fake is the whole process table. No
//! test here lets a Claude adopt SUCCEED: that path edits the operator's real
//! Claude settings.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pij_core::events::EventFilter;
use pij_core::model::{Event, Harness, Pane, PaneProcess, ProcIdentity, SeatDescriptor, Seq};
use pij_core::ports::{LivenessPort, Registry, SeatFilter, Spine};
use pij_testkit::fakes::{FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use super::tests::{config, spawn, test_services};
use super::*;

const CODE: &str = "E-RS-CONVERSATION-LIVE";
const KIND: &str = "seat.double-detected";
const LIVE_SEAT: &str = "pij-different-smelt";
const LIVE_PANE: &str = "%5";
const STRAY_PANE: &str = "%7";
const LIVE_PID: u32 = 3_999_951;
const LIVE_START: u64 = 20_261_015_005_734;
const STRAY_PID: u32 = 3_999_971;
/// A stale record's pid: a process the liveness fake knows, on the LIVE pane.
const STALE_PID: u32 = 3_999_991;
const SESSION: &str = "cc2d2e8f-1740-4000-8000-000000000174";
/// Claude's own `procStart`: UTC, C locale. 2026-10-15 is a Thursday.
const RECORD_UTC: &str = "Thu Oct 15 12:00:00 2026";
const FOLDER: &str = "/abs/tree";
/// Today's text, byte for byte. A pane with no double keeps it.
const NOT_REGISTERED: &str = "no live seat in the rs store owns pane `%7` — this seat is not \
                              registered here. Run `pij adopt \"$TMUX_PANE\" --harness <h>` first.";

/// [`RECORD_UTC`] as this machine's packed local wall time, which is what the
/// liveness port reports for the process that wrote the record.
fn stray_start() -> u64 {
    let offset = i64::from(pij_harnesses::proc::local_utc_offset_minutes().expect("utc offset"));
    let minutes = 12 * 60 + offset;
    let day = 15 + minutes.div_euclid(1_440);
    let minute = minutes.rem_euclid(1_440);
    u64::try_from(
        20_261_000_000_000 + day * 1_000_000 + (minute / 60) * 10_000 + (minute % 60) * 100,
    )
    .expect("packed start")
}

/// The liveness fake, recording every pid it is asked about.
struct Probes {
    inner: FakeLiveness,
    asked: Mutex<Vec<u32>>,
}

#[async_trait]
impl LivenessPort for Probes {
    async fn proc_start(&self, pid: u32) -> pij_core::error::Result<Option<u64>> {
        self.asked.lock().expect("probe log").push(pid);
        self.inner.proc_start(pid).await
    }
}

struct World {
    addr: SocketAddr,
    registry: Arc<FakeRegistry>,
    bus: Arc<crate::events::EventBus>,
    probes: Arc<Probes>,
    server: tokio::task::JoinHandle<()>,
}

fn live_seat() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(LIVE_SEAT, Harness::Claude, FOLDER);
    seat.pane = Some(LIVE_PANE.to_string());
    seat.proc = Some(ProcIdentity {
        pid: LIVE_PID,
        proc_start: LIVE_START,
    });
    seat.harness_session = Some(SESSION.to_string());
    seat
}

/// A Claude home holding one process record per `(pid, pane)`, all running
/// [`SESSION`] and all started at [`RECORD_UTC`].
fn claude_home(records: &[(u32, &str)]) -> PathBuf {
    let home = pij_testkit::fresh_dir("pij-double-seat-claude");
    std::fs::create_dir_all(home.join("sessions")).expect("sessions dir");
    for (pid, pane) in records {
        std::fs::write(
            home.join(format!("sessions/{pid}.json")),
            json!({
                "pid": pid, "sessionId": SESSION, "procStart": RECORD_UTC,
                "tmux": format!("work:@1.{pane}"),
            })
            .to_string(),
        )
        .expect("process record");
    }
    home
}

/// Seat `pij-different-smelt` is live in %5. Pane %7 runs pid 3999971, which
/// the liveness fake reports as started at `stray_observed`.
async fn world(spine: Arc<FakeSpine>, home: PathBuf, stray_observed: u64) -> World {
    let registry = Arc::new(FakeRegistry::new().with_seat(live_seat()));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine,
    )
    .await;
    let pane = |id: &str| Pane {
        id: id.to_string(),
        session: "work".to_string(),
        window: "@1".to_string(),
        title: "claude".to_string(),
        cursor_x: None,
        cursor_y: None,
    };
    let process = |pid| PaneProcess {
        pid,
        cwd: FOLDER.to_string(),
    };
    services.tmux = Arc::new(
        FakeTmux::new()
            .with_pane(pane(LIVE_PANE))
            .with_pane_process(LIVE_PANE, process(LIVE_PID))
            .with_pane(pane(STRAY_PANE))
            .with_pane_process(STRAY_PANE, process(STRAY_PID)),
    );
    let probes = Arc::new(Probes {
        inner: FakeLiveness::new()
            .with_proc(ProcIdentity {
                pid: LIVE_PID,
                proc_start: LIVE_START,
            })
            .with_proc(ProcIdentity {
                pid: STRAY_PID,
                proc_start: stray_observed,
            })
            .with_proc(ProcIdentity {
                pid: STALE_PID,
                proc_start: stray_start(),
            }),
        asked: Mutex::default(),
    });
    services.liveness = probes.clone();
    services.claude_homes = vec![home].into();
    let bus = services.event_bus.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    World {
        addr,
        registry,
        bus,
        probes,
        server,
    }
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth("key")
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    let status = response.status().as_u16();
    let text = response.text().await.expect("response body");
    let body = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{path} answered a non-envelope ({error}): {text}"));
    (status, body)
}

async fn adopt_stray(addr: SocketAddr) -> (u16, Value) {
    post(
        addr,
        "/v1/adopt",
        json!({"argv": ["adopt", STRAY_PANE, "--harness", "claude"]}),
    )
    .await
}

/// Every verb that resolves its caller from the pane, called from %7.
fn stray_verbs() -> Vec<(&'static str, Value)> {
    let caller = json!({ "TMUX_PANE": STRAY_PANE });
    vec![
        ("/v1/whoami", json!({ "caller": caller })),
        (
            "/v1/report",
            json!({"caller": caller, "argv": ["report", "state", "ready"]}),
        ),
        (
            "/v1/shim/send",
            json!({"argv": ["send", LIVE_SEAT, "hi"], "caller": caller}),
        ),
        (
            "/v1/bg",
            json!({"title": "t", "command": "true", "caller": caller}),
        ),
        (
            "/v1/spawn",
            json!({"harness": "claude", "cwd": FOLDER, "role": "worker", "caller": caller}),
        ),
        (
            "/v1/activity",
            json!({"pane": STRAY_PANE, "native_session": SESSION, "state": "working"}),
        ),
        (
            "/v1/fyi/claim",
            json!({"pane": STRAY_PANE, "native_session": SESSION, "via": "hook:claude"}),
        ),
    ]
}

/// The coded refusal: code first, the live seat and its pane, the fix, and the
/// machine-readable details a script branches on.
fn assert_double(what: &str, status: u16, body: &Value) {
    let meta = body["meta"].as_str().unwrap_or_default();
    assert!(
        meta.starts_with(CODE),
        "{what} must lead with {CODE}, got {status}: {body}"
    );
    assert!(
        meta.contains(&format!(
            "conversation `{SESSION}` is live as seat `{LIVE_SEAT}` in pane {LIVE_PANE}"
        )),
        "{what} names the conversation, the live seat and its pane: {meta}"
    );
    assert!(
        meta.contains(&format!(
            "close one copy, then run `pij-rs adopt {STRAY_PANE} --harness claude`"
        )),
        "{what} names the fix: {meta}"
    );
    assert_eq!(status, 409, "{what}: a conflict with a live seat: {body}");
    assert_eq!(body["ok"], json!(false), "{what}: {body}");
    assert!(
        body["error"].is_string(),
        "{what} carries a branchable error kind: {body}"
    );
    assert_eq!(
        body["details"],
        json!({
            "code": CODE, "seat": LIVE_SEAT, "pane": LIVE_PANE,
            "harness_session": SESSION, "pid": LIVE_PID,
        }),
        "{what}: details"
    );
}

/// The `seat.double-detected` audit records, oldest first.
async fn notices(world_spine: &FakeSpine) -> Vec<Value> {
    world_spine
        .tail(None, Seq(0))
        .await
        .expect("spine tail")
        .into_iter()
        .filter(|event| event.kind == KIND)
        .map(|event| {
            assert_eq!(
                event.seat.as_ref().map(|seat| seat.as_str()),
                Some(LIVE_SEAT),
                "the record is filed under the live seat"
            );
            serde_json::from_str(&event.payload).expect("notice payload")
        })
        .collect()
}

/// Wait, bounded, for message `id` to be delivered: any event other than the
/// audit record that carries its id.
async fn delivery_of(live: &mut crate::events::Subscription, id: &str) -> Event {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = live.next().await.expect("event bus open");
            if event.kind != KIND && event.payload.contains(id) {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("notice {id} was never delivered to {LIVE_SEAT}"))
}

fn assert_notice(notice: &Value) {
    assert_eq!(notice["seat"], json!(LIVE_SEAT), "{notice}");
    assert_eq!(notice["stray_pane"], json!(STRAY_PANE), "{notice}");
    assert_eq!(notice["stray_pid"], json!(STRAY_PID), "{notice}");
    assert_eq!(notice["proc_start"], json!(stray_start()), "{notice}");
    assert_eq!(notice["harness_session"], json!(SESSION), "{notice}");
    let body = notice["body"].as_str().unwrap_or_default();
    for part in [
        STRAY_PANE.to_string(),
        format!("pid {STRAY_PID}"),
        format!("pij-rs adopt {STRAY_PANE} --harness claude"),
    ] {
        assert!(body.contains(&part), "the notice names `{part}`: {notice}");
    }
}

#[tokio::test]
async fn double_seat_adopt_from_the_stray_pane_is_refused_with_the_code_and_details() {
    let world = world(
        Arc::new(FakeSpine::new()),
        claude_home(&[(STRAY_PID, STRAY_PANE)]),
        stray_start(),
    )
    .await;

    let (status, body) = adopt_stray(world.addr).await;

    assert_double("adopt", status, &body);
    assert!(
        body["meta"]
            .as_str()
            .unwrap_or_default()
            .contains("from the copy you keep"),
        "{body}"
    );
    let seats = world
        .registry
        .list(SeatFilter::default())
        .await
        .expect("list");
    assert_eq!(seats, vec![live_seat()], "nothing was written");
    world.server.abort();
}

#[tokio::test]
async fn double_seat_every_pane_resolved_verb_from_the_stray_pane_names_the_live_seat() {
    let world = world(
        Arc::new(FakeSpine::new()),
        claude_home(&[(STRAY_PID, STRAY_PANE)]),
        stray_start(),
    )
    .await;

    for (path, body) in stray_verbs() {
        let (status, answer) = post(world.addr, path, body).await;
        assert_double(path, status, &answer);
    }
    let seats = world
        .registry
        .list(SeatFilter::default())
        .await
        .expect("list");
    assert_eq!(seats, vec![live_seat()], "no verb wrote anything");
    world.server.abort();
}

/// rk-0002: no record on the pane, or a record whose recorded start does not
/// match the live process (a recycled pid's stale file), raises no double.
#[tokio::test]
async fn double_seat_without_a_matching_record_keeps_the_not_registered_text() {
    for (case, home, observed) in [
        ("no record", claude_home(&[]), stray_start()),
        (
            "mismatched start",
            claude_home(&[(STRAY_PID, STRAY_PANE)]),
            stray_start() + 1,
        ),
    ] {
        let spine = Arc::new(FakeSpine::new());
        let world = world(spine.clone(), home, observed).await;
        for (path, body) in stray_verbs() {
            let (status, answer) = post(world.addr, path, body).await;
            let meta = answer["meta"].as_str().unwrap_or_default();
            if matches!(path, "/v1/activity" | "/v1/fyi/claim") {
                assert_eq!(status, 404, "{case} {path}: {answer}");
                assert_eq!(meta, "no live seat is bound to pane %7", "{case} {path}");
            } else {
                assert_eq!(status, 400, "{case} {path}: {answer}");
                assert_eq!(meta, NOT_REGISTERED, "{case} {path}");
            }
            assert!(answer.get("details").is_none(), "{case} {path}: {answer}");
        }
        assert!(notices(&spine).await.is_empty(), "{case}: no notice");
        world.server.abort();
    }
}

/// rk-0001: the scan runs only on the refusal path. A pane that resolves to a
/// seat never reads the process records or probes a pid they name.
#[tokio::test]
async fn double_seat_a_resolved_pane_never_reads_records_or_probes_them() {
    let spine = Arc::new(FakeSpine::new());
    let world = world(
        spine.clone(),
        claude_home(&[(STALE_PID, LIVE_PANE), (STRAY_PID, STRAY_PANE)]),
        stray_start(),
    )
    .await;

    let caller = json!({ "TMUX_PANE": LIVE_PANE });
    let (status, whoami) = post(world.addr, "/v1/whoami", json!({ "caller": caller })).await;
    assert_eq!(status, 200, "{whoami}");
    assert_eq!(whoami["data"]["id"], json!(LIVE_SEAT));
    let (status, activity) = post(
        world.addr,
        "/v1/activity",
        json!({"pane": LIVE_PANE, "native_session": SESSION, "state": "working"}),
    )
    .await;
    assert_eq!(status, 200, "{activity}");

    let asked = world.probes.asked.lock().expect("probe log").clone();
    assert!(
        !asked.contains(&STALE_PID) && !asked.contains(&STRAY_PID),
        "a resolved pane probed a record's pid: {asked:?}"
    );
    assert!(notices(&spine).await.is_empty());
    world.server.abort();
}

/// ac-0006 and rk-0003: one notice per (live seat, stray pid + start), however
/// many refusals and whichever verb, and a restarted daemon does not resend.
#[tokio::test]
async fn double_seat_notice_goes_to_the_live_seat_once_per_pair_even_across_a_restart() {
    let spine = Arc::new(FakeSpine::new());
    let home = claude_home(&[(STRAY_PID, STRAY_PANE)]);
    let first = world(spine.clone(), home.clone(), stray_start()).await;
    let mut live = first.bus.subscribe_live(EventFilter::all());

    let (status, body) = adopt_stray(first.addr).await;
    assert_double("adopt", status, &body);
    let recorded = notices(&spine).await;
    assert_eq!(recorded.len(), 1, "one notice: {recorded:?}");
    assert_notice(&recorded[0]);
    let id = recorded[0]["msg_id"].as_str().expect("notice msg_id");
    delivery_of(&mut live, id).await;

    for (path, body) in stray_verbs() {
        let (status, answer) = post(first.addr, path, body).await;
        assert_double(path, status, &answer);
    }
    adopt_stray(first.addr).await;
    assert_eq!(notices(&spine).await.len(), 1, "a repeat sends nothing");
    first.server.abort();

    // A restart: a new daemon over the same spine.
    let second = world(spine.clone(), home, stray_start()).await;
    let (status, body) = adopt_stray(second.addr).await;
    assert_double("adopt after restart", status, &body);
    assert_eq!(
        notices(&spine).await.len(),
        1,
        "the spine record survives the restart"
    );
    second.server.abort();
}

/// ac-0006: the dedupe is a brake. A spine read that fails counts as sent.
#[tokio::test]
async fn double_seat_an_unreadable_spine_counts_as_already_sent() {
    let spine = Arc::new(FakeSpine::new());
    let world = world(
        spine.clone(),
        claude_home(&[(STRAY_PID, STRAY_PANE)]),
        stray_start(),
    )
    .await;
    spine.script_latest_matching_error("spine unavailable");

    let (status, body) = adopt_stray(world.addr).await;

    assert_double("adopt", status, &body);
    assert!(
        notices(&spine).await.is_empty(),
        "an unknown dedupe answer sends nothing"
    );
    world.server.abort();
}

/// ac-0008(d): the 2026-10-15 incident, end to end.
#[tokio::test]
async fn double_seat_incident_end_to_end() {
    let spine = Arc::new(FakeSpine::new());
    let world = world(
        spine.clone(),
        claude_home(&[(STRAY_PID, STRAY_PANE)]),
        stray_start(),
    )
    .await;
    let mut live = world.bus.subscribe_live(EventFilter::all());

    // Pane %7's SessionStart hook adopts and is refused with the code.
    let (status, refusal) = adopt_stray(world.addr).await;
    assert_double("adopt from %7", status, &refusal);

    // The live seat is told once.
    let recorded = notices(&spine).await;
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_notice(&recorded[0]);
    let id = recorded[0]["msg_id"].as_str().expect("notice msg_id");
    let delivered = delivery_of(&mut live, id).await;
    assert_eq!(
        delivered.seat.as_ref().map(|seat| seat.as_str()),
        Some(LIVE_SEAT)
    );

    // Jordan, in %7, sends: refused with the actionable text, not "not registered".
    let (status, send) = post(
        world.addr,
        "/v1/shim/send",
        json!({"argv": ["send", LIVE_SEAT, "hi"], "caller": {"TMUX_PANE": STRAY_PANE}}),
    )
    .await;
    assert_double("send from %7", status, &send);

    // The status line's heal adopts again: no second notice.
    adopt_stray(world.addr).await;
    assert_eq!(notices(&spine).await.len(), 1);

    // Every Claude script branches on the code the daemon actually emits.
    let code = refusal["details"]["code"].as_str().expect("code");
    for script in [
        "claude-session-start-pij.sh",
        "claude-user-prompt-submit-pij.sh",
        "claude-statusline-pij.sh",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../harness/scripts")
            .join(script);
        let text = std::fs::read_to_string(&path).expect("script");
        assert!(text.contains(code), "{script} must branch on {code}");
    }
    world.server.abort();
}
