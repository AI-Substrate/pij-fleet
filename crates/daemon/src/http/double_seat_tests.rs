//! Plan 174 (pij-fleet#55) — one conversation running as two seats.
//!
//! The 2026-10-15 incident, on fixtures: seat `pij-different-smelt` is live in
//! pane %5, and a hand-resumed second copy of its conversation runs in pane %7.
//! Every arm reads a FIXTURE Claude home, fake tmux and fake liveness; nothing
//! here reads the operator's `~/.claude`, tmux server or daemon.
//!
//! The code and the spine kind are spelled as literals on purpose: these tests
//! were written red against main bb9f0b1, where neither exists in Rust yet.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::model::{Harness, Pane, PaneProcess, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{LivenessPort, Registry, Spine};
use pij_testkit::fakes::{FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux};
use serde_json::{Value, json};

use super::tests::{config, spawn, test_services};
use super::*;

const CODE: &str = "E-RS-CONVERSATION-LIVE";
const DETECTED: &str = "seat.double-detected";
const SESSION: &str = "cc2d2e8f-0174-4000-8000-00000000d0b1";
const SEAT: &str = "pij-different-smelt";
const LIVE_PANE: &str = "%5";
const STRAY_PANE: &str = "%7";
const LIVE_PID: u32 = 917_405;
const STRAY_PID: u32 = 917_407;

/// The local packed start `liveness` reports for a process Claude recorded as
/// starting at `hh:mm:ss` UTC on 2026-10-15, at this host's real UTC offset —
/// the offset the daemon itself reads.
fn local_start(hh: i64, mm: i64, ss: u64) -> u64 {
    let offset = i64::from(pij_harnesses::proc::local_utc_offset_minutes().expect("utc offset"));
    let minutes = hh * 60 + mm + offset;
    let day = 15 + minutes.div_euclid(1_440);
    let minute_of_day = minutes.rem_euclid(1_440);
    20_261_000_000_000
        + u64::try_from(day).unwrap() * 1_000_000
        + u64::try_from(minute_of_day / 60).unwrap() * 10_000
        + u64::try_from(minute_of_day % 60).unwrap() * 100
        + ss
}

fn live_proc() -> ProcIdentity {
    ProcIdentity {
        pid: LIVE_PID,
        proc_start: local_start(0, 57, 34),
    }
}

fn stray_proc() -> ProcIdentity {
    ProcIdentity {
        pid: STRAY_PID,
        proc_start: local_start(0, 58, 23),
    }
}

/// A fixture Claude home holding `sessions/<pid>.json` records.
struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let home = pij_testkit::fresh_dir("pij-double-seat-home");
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        Self(home)
    }

    /// Claude's own record: pid, conversation, UTC start, tmux location.
    fn record(&self, pid: u32, session: &str, utc_start: &str, pane: &str) {
        std::fs::write(
            self.0.join(format!("sessions/{pid}.json")),
            json!({
                "pid": pid, "sessionId": session, "procStart": utc_start,
                "tmux": format!("main:@1.{pane}"),
            })
            .to_string(),
        )
        .unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Liveness that remembers every pid it was asked about (rk-0001).
struct Probed {
    inner: FakeLiveness,
    asked: Mutex<Vec<u32>>,
}

#[async_trait]
impl LivenessPort for Probed {
    async fn proc_start(&self, pid: u32) -> pij_core::error::Result<Option<u64>> {
        self.asked.lock().unwrap().push(pid);
        self.inner.proc_start(pid).await
    }
}

impl Probed {
    fn asked(&self) -> Vec<u32> {
        self.asked.lock().unwrap().clone()
    }

    fn forget(&self) {
        self.asked.lock().unwrap().clear();
    }
}

struct World {
    addr: SocketAddr,
    registry: Arc<FakeRegistry>,
    spine: Arc<FakeSpine>,
    liveness: Arc<Probed>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for World {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn pane(id: &str) -> Pane {
    Pane {
        id: id.to_string(),
        session: "main".to_string(),
        window: "@1".to_string(),
        title: "claude".to_string(),
        cursor_x: None,
        cursor_y: None,
    }
}

/// The incident machine: `SEAT` live in %5, a second Claude process in %7.
/// `registry` and `spine` survive across calls, so a second `world` on the same
/// pair is a daemon restart.
async fn world_on(
    registry: Arc<FakeRegistry>,
    spine: Arc<FakeSpine>,
    home: &Home,
    stray: ProcIdentity,
    extra: &[ProcIdentity],
) -> World {
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine.clone(),
    )
    .await;
    services.tmux = Arc::new(
        FakeTmux::new()
            .with_pane(pane(LIVE_PANE))
            .with_pane(pane(STRAY_PANE))
            .with_pane_process(
                STRAY_PANE,
                PaneProcess {
                    pid: stray.pid,
                    cwd: "/abs/tree".to_string(),
                },
            ),
    );
    let mut fake = FakeLiveness::new().with_proc(live_proc()).with_proc(stray);
    for proc in extra {
        fake = fake.with_proc(*proc);
    }
    let liveness = Arc::new(Probed {
        inner: fake,
        asked: Mutex::new(Vec::new()),
    });
    services.liveness = liveness.clone();
    services.claude_homes = Arc::from([home.0.clone()]);
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    World {
        addr,
        registry,
        spine,
        liveness,
        server,
    }
}

/// A registry holding only `SEAT`, live in %5 on its conversation.
async fn seeded_registry() -> Arc<FakeRegistry> {
    let registry = Arc::new(FakeRegistry::new());
    let mut seat = SeatDescriptor::new(SEAT, Harness::Claude, "/abs/tree");
    seat.pane = Some(LIVE_PANE.to_string());
    seat.proc = Some(live_proc());
    seat.harness_session = Some(SESSION.to_string());
    registry.put(seat).await.unwrap();
    registry
}

/// Seed the live seat and both Claude records, then boot.
async fn incident() -> (World, Home) {
    let registry = seeded_registry().await;
    let home = Home::new();
    home.record(LIVE_PID, SESSION, "Thu Oct 15 00:57:34 2026", LIVE_PANE);
    home.record(STRAY_PID, SESSION, "Thu Oct 15 00:58:23 2026", STRAY_PANE);
    let world = world_on(
        registry,
        Arc::new(FakeSpine::new()),
        &home,
        stray_proc(),
        &[],
    )
    .await;
    (world, home)
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> Value {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth("key")
        .json(&body)
        .send()
        .await
        .expect("request");
    let text = response.text().await.expect("body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}: {text}"))
}

async fn adopt_stray(addr: SocketAddr) -> Value {
    post(
        addr,
        "/v1/adopt",
        json!({"argv": ["adopt", STRAY_PANE, "--harness", "claude"]}),
    )
    .await
}

fn meta(body: &Value) -> &str {
    body["meta"].as_str().unwrap_or_default()
}

/// The coded, actionable refusal every verb from the stray pane must give.
fn assert_double_refusal(verb: &str, body: &Value) {
    assert_eq!(body["ok"], false, "{verb}: {body}");
    let text = meta(body);
    assert!(
        text.starts_with(CODE),
        "{verb}: the code leads the text: {body}"
    );
    for needle in [
        format!("conversation `{SESSION}` is live as seat `{SEAT}`"),
        format!("(pane {LIVE_PANE})"),
        "close one copy".to_string(),
        format!("pij-rs adopt {STRAY_PANE} --harness claude"),
    ] {
        assert!(text.contains(&needle), "{verb}: names `{needle}`: {body}");
    }
    assert_eq!(body["details"]["code"], CODE, "{verb}: {body}");
    assert_eq!(body["details"]["seat"], SEAT, "{verb}: {body}");
    assert_eq!(body["details"]["pane"], LIVE_PANE, "{verb}: {body}");
    assert_eq!(
        body["details"]["harness_session"], SESSION,
        "{verb}: {body}"
    );
    assert_eq!(body["details"]["pid"], LIVE_PID, "{verb}: {body}");
}

fn not_registered(pane: &str) -> String {
    format!(
        "no live seat in the rs store owns pane `{pane}` — this seat is not registered \
         here. Run `pij adopt \"$TMUX_PANE\" --harness <h>` first."
    )
}

async fn detections(spine: &FakeSpine) -> Vec<Value> {
    spine
        .tail(Some(&SeatId::from(SEAT)), Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == DETECTED)
        .map(|event| serde_json::from_str(&event.payload).unwrap())
        .collect()
}

/// AC1 — adopt from the stray pane: the stable code leads the refusal, the
/// details carry the live copy, and the text names the fix. Nothing is written.
#[tokio::test]
async fn double_seat_adopt_refusal_carries_the_code_details_and_fix() {
    let (world, _home) = incident().await;

    let body = adopt_stray(world.addr).await;

    assert_double_refusal("adopt", &body);
    assert!(
        meta(&body).contains("in another process"),
        "keeps naming the other process: {body}"
    );
    let live: Vec<_> = world
        .registry
        .list(Default::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .collect();
    assert_eq!(live.len(), 1, "the refusal wrote no seat: {live:?}");
    assert_eq!(
        live[0].proc,
        Some(live_proc()),
        "the live seat is untouched"
    );
}

/// AC2 — every verb resolved by pane, from the pane no live seat owns, gives
/// the same coded refusal. The unchanged-text and no-scan pins are their own
/// tests below.
#[tokio::test]
async fn double_seat_stray_pane_verbs_refuse_with_one_code_and_actionable_text() {
    let (world, _home) = incident().await;
    let addr = world.addr;

    let whoami = post(addr, "/v1/whoami", json!({"pane": STRAY_PANE})).await;
    assert_double_refusal("whoami --pane", &whoami);
    let send = post(
        addr,
        "/v1/shim/send",
        json!({"argv": ["send", SEAT, "hello"], "caller": {"tmuxPane": STRAY_PANE}}),
    )
    .await;
    assert_double_refusal("send", &send);
    let activity = post(
        addr,
        "/v1/activity",
        json!({"pane": STRAY_PANE, "state": "working"}),
    )
    .await;
    assert_double_refusal("activity", &activity);
    let claim = post(
        addr,
        "/v1/fyi/claim",
        json!({"pane": STRAY_PANE, "via": "hook:claude"}),
    )
    .await;
    assert_double_refusal("fyi-claim", &claim);
}

/// PIN (green on base by design; plan 174 says this must stay UNCHANGED): a
/// pane no live seat owns, without a PROVEN second copy, keeps today's
/// not-registered text byte for byte. Three unproven cases: no record for the
/// pane; a record whose pid was recycled (its observed start disagrees,
/// rk-0002); a record whose conversation no live seat holds.
#[tokio::test]
async fn double_seat_pin_unproven_stray_keeps_the_not_registered_text() {
    let home = Home::new();
    home.record(LIVE_PID, SESSION, "Thu Oct 15 00:57:34 2026", LIVE_PANE);
    const RECYCLED_PID: u32 = 917_409;
    const OTHER_PID: u32 = 917_410;
    home.record(RECYCLED_PID, SESSION, "Thu Oct 15 00:59:00 2026", "%9");
    home.record(
        OTHER_PID,
        "some-other-conversation",
        "Thu Oct 15 01:00:00 2026",
        "%10",
    );
    let world = world_on(
        seeded_registry().await,
        Arc::new(FakeSpine::new()),
        &home,
        stray_proc(),
        &[
            ProcIdentity {
                pid: RECYCLED_PID,
                proc_start: local_start(0, 59, 1),
            },
            ProcIdentity {
                pid: OTHER_PID,
                proc_start: local_start(1, 0, 0),
            },
        ],
    )
    .await;
    for pane in ["%8", "%9", "%10"] {
        let body = post(world.addr, "/v1/whoami", json!({"pane": pane})).await;
        assert_eq!(body["ok"], false, "{pane}: {body}");
        assert_eq!(meta(&body), not_registered(pane), "{pane}: {body}");
    }
}

/// PIN (green on base by design, rk-0001): a pane a live seat owns resolves
/// without reading the Claude session records. The decoy record names that
/// pane; probing its pid would be the scan.
#[tokio::test]
async fn double_seat_pin_resolved_pane_never_reads_session_records() {
    let (world, home) = incident().await;
    const DECOY_PID: u32 = 917_411;
    home.record(DECOY_PID, SESSION, "Thu Oct 15 01:01:00 2026", LIVE_PANE);
    world.liveness.forget();
    let resolved = post(world.addr, "/v1/whoami", json!({"pane": LIVE_PANE})).await;
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert!(
        !world.liveness.asked().contains(&DECOY_PID),
        "a resolved pane read the Claude session records: {:?}",
        world.liveness.asked()
    );
}

/// AC6 — the live seat is told once per (seat, stray pid, stray start): the
/// spine event is written with the pair, the notice is delivered, and repeat
/// refusals from any verb, before or after a daemon restart, send nothing more.
/// A new stray process is a new pair.
#[tokio::test]
async fn double_seat_notice_reaches_the_live_seat_once_per_pair_across_restart() {
    let (world, home) = incident().await;

    assert_double_refusal("adopt", &adopt_stray(world.addr).await);
    let first = detections(&world.spine).await;
    assert_eq!(first.len(), 1, "one detection recorded: {first:?}");
    let event = &first[0];
    assert_eq!(event["seat"], SEAT, "{event}");
    assert_eq!(event["stray_pane"], STRAY_PANE, "{event}");
    assert_eq!(event["stray_pid"], STRAY_PID, "{event}");
    assert_eq!(event["proc_start"], stray_proc().proc_start, "{event}");
    assert_eq!(event["harness_session"], SESSION, "{event}");
    let msg_id = event["msg_id"]
        .as_str()
        .expect("the notice's id")
        .to_string();
    let delivered: Vec<_> = world
        .spine
        .tail(Some(&SeatId::from(SEAT)), Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind != DETECTED && event.payload.contains(&msg_id))
        .collect();
    assert!(
        !delivered.is_empty(),
        "the notice went through the ordinary delivery path"
    );

    // Repeats from every surface: nothing more.
    assert_double_refusal("adopt", &adopt_stray(world.addr).await);
    for (path, body) in [
        ("/v1/whoami", json!({"pane": STRAY_PANE})),
        ("/v1/activity", json!({"pane": STRAY_PANE, "state": "idle"})),
    ] {
        assert_double_refusal(path, &post(world.addr, path, body).await);
    }
    assert_eq!(detections(&world.spine).await.len(), 1, "no repeat notice");

    // Restart: same registry and spine, a fresh daemon.
    let (registry, spine) = (world.registry.clone(), world.spine.clone());
    drop(world);
    let world = world_on(registry.clone(), spine.clone(), &home, stray_proc(), &[]).await;
    assert_double_refusal("adopt", &adopt_stray(world.addr).await);
    assert_eq!(
        detections(&world.spine).await.len(),
        1,
        "the spine event is the dedupe record, so a restart does not resend"
    );

    // Claude restarted in the stray pane: a new pid, a new pair, one new notice.
    const NEXT_PID: u32 = 917_408;
    let next = ProcIdentity {
        pid: NEXT_PID,
        proc_start: local_start(1, 2, 3),
    };
    std::fs::remove_file(home.0.join(format!("sessions/{STRAY_PID}.json"))).unwrap();
    home.record(NEXT_PID, SESSION, "Thu Oct 15 01:02:03 2026", STRAY_PANE);
    drop(world);
    let world = world_on(registry, spine, &home, next, &[]).await;
    assert_double_refusal("adopt", &adopt_stray(world.addr).await);
    let all = detections(&world.spine).await;
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1]["stray_pid"], NEXT_PID);
}

/// AC6 — the dedupe is a BRAKE: when the spine cannot say whether the notice
/// was sent, it counts as sent. The refusal itself is unaffected.
#[tokio::test]
async fn double_seat_notice_counts_as_sent_when_the_dedupe_read_fails() {
    let (world, _home) = incident().await;
    world
        .spine
        .script_latest_matching_error("spine unavailable for the dedupe read");

    assert_double_refusal("adopt", &adopt_stray(world.addr).await);

    assert!(
        detections(&world.spine).await.is_empty(),
        "fail-safe: no notice and no detection record on an unknown read"
    );
}
