//! Cross-language contract proof, not real Copilot/SDK/model acceptance.
//! Set PIJ_NATIVE_RUNTIME_MODULE to the exact runtime store.mjs in split worktrees.
//! Same-native-session fast restart is governed by plan149 registration tests,
//! not the superseded live-old-process veto that used to sit in this probe.
//! After composition the default is repo/.copilot/extensions/pij/store.mjs.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::delivery::delivery_kind;
use pij_core::model::{JobId, Pane, PaneProcess, ProcIdentity, SeatId, SemanticState};
use pij_core::ports::{LivenessPort, Queue, Registry};
use pij_daemon::delivery::DeliveryService;
use pij_daemon::http::router;
use pij_harnesses::proc::ProcLiveness;
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeTmux;
use serde_json::{Value, json};

const KEY: &str = "native-cross-language-key";
const SESSION: &str = "native-cross-language-cold-resume";
const BODY: &str = "M1 accepted before ACK must not be injected twice";

// Only native.send is a test acceptance boundary. Chooser, HTTP client,
// registration readback, delivery/dedupe, and filesystem journal are production.
const RUNNER: &str = r#"
import assert from 'node:assert/strict';
import { appendFile, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const input = JSON.parse(process.argv[1]);
assert.ok(typeof AbortSignal.any === 'function', 'Node must support AbortSignal.any');
const signal = AbortSignal.timeout(4000);
const client = new DaemonClient({ addr: input.addr, readKey: async () => input.key });
const roster = await client.request('/v1/seats?scope=local', undefined, signal);
assert.ok(Array.isArray(roster.seats));
const registration = await resolveRegistration(client, chooseRegistration({
    sessionId: input.session, host: input.host, folder: input.folder,
    seats: roster.seats, env: {},
}), signal);
const tuple = {
    seat: registration.id, native_session: registration.harness_session,
    pid: registration.pid, proc_start: registration.proc_start,
};
if (input.previous) {
    assert.equal(registration.id, input.previous.registration.id,
        'actual chooser must nominate original address across changed host');
    assert.notEqual(registration.pid, input.previous.registration.pid);
    assert.equal(registration.supersedes, undefined, 'new process cannot supersede');
}
const moduleDigest = createHash('sha256').update(await readFile(input.module)).digest('hex');
    const journal = new FileJournal(input.stateDir, registration);
    const acceptanceLog = join(input.stateDir, 'native-send-boundary.jsonl');
    const events = [];
    const consumedEvent = { type: 'user.message', id: 'test-consumed-event',
        data: { messageId: 'test-native-acceptance-not-model-completion' } };
    const nativeHistory = input.stage === 'resume' ? [consumedEvent] : [];
    const native = {
        rpc: { eventLog: {
            async tail() { return { cursor: String(nativeHistory.length) }; },
            async read({ cursor = '0', max }) {
                const events = nativeHistory.slice(Number(cursor), Number(cursor) + max);
                const next = Number(cursor) + events.length;
                return { cursor: String(next), events, hasMore: next < nativeHistory.length, cursorStatus: 'ok' };
            },
        } },
        on() { return () => {}; },
        async send(message) {
            assert.equal(input.stage, 'initial', 'accepted replay must never call native.send');
            assert.equal(message.mode, 'immediate');
            assert.ok(message.prompt.endsWith(input.body));
            await appendFile(acceptanceLog, JSON.stringify(message) + '\n', { mode: 0o600 });
            nativeHistory.push(consumedEvent);
            return 'test-native-acceptance-not-model-completion';
        },
    };
    const bridge = new NativeBridge({ registration, native, client, journal, report: e => events.push(e) });
    try {
        await bridge.register();
        // Rolling-upgrade contract: even the unchanged current-main receiver
        // must receive while its seat reports Hold.
        await client.request('/v1/report', {
            seat: registration.id, argv: ['report', 'state', 'hold'],
        }, signal);
        if (input.stage === 'initial') {
            const receipt = await client.request('/v1/send', {
                from: 'pij-contract-peer', to: { seat: registration.id },
                body: input.body, msg_id: 'cross-language-original-m1',
            }, signal);
            assert.equal(receipt.outcome.outcome, 'queued');
        } else {
            assert.equal(journal.directory, input.previous.journalDirectory);
            const record = await journal.load(input.previous.messageId);
            assert.equal(record.state, 'accepted');
            assert.equal(record.nativeId, 'test-native-acceptance-not-model-completion');
        }
        const page = await client.claimInbox(tuple, signal);
        assert.equal(page.hold, null);
        assert.equal(page.claims.length, 1);
        const claim = page.claims[0];
        assert.equal(claim.message.body, input.body);
        if (input.previous) {
            assert.equal(claim.job_id, input.previous.jobId);
            assert.equal(claim.message.msg_id, input.previous.messageId);
        }
        if (input.stage === 'initial') {
            const request = client.request.bind(client);
            const interrupted = new Error('test crash after durable acceptance before HTTP ACK');
            client.request = async (path, body, requestSignal) => {
                if (path !== '/v1/inbox/ack') return request(path, body, requestSignal);
                const record = await journal.load(claim.message.msg_id);
                assert.equal(record.state, 'accepted');
                assert.equal(body.job_id, claim.job_id);
                assert.equal(body.pid, registration.pid);
                throw interrupted;
            };
            await assert.rejects(bridge.deliver(claim), error => error === interrupted);
            assert.equal(events.filter(e => e.kind === 'native-accepted').length, 1);
            assert.equal(events.filter(e => e.kind === 'inbox-acknowledged').length, 0);
        } else {
            await bridge.deliver(claim);
            assert.equal(events.filter(e => e.kind === 'native-accepted').length, 0);
            assert.equal(events.filter(e => e.kind === 'inbox-acknowledged').length, 1);
        }
        const acceptedBytes = await readFile(journal.path(claim.message.msg_id));
        assert.equal((await journal.load(claim.message.msg_id)).state, 'accepted');
        assert.equal((await readFile(acceptanceLog, 'utf8')).trim().split('\n').length, 1);
        console.log(JSON.stringify({
            reportedState: 'hold',
            registration, moduleDigest, jobId: claim.job_id, messageId: claim.message.msg_id,
            journalDirectory: journal.directory, journalPath: journal.path(claim.message.msg_id),
            journalDigest: createHash('sha256').update(acceptedBytes).digest('hex'),
            acceptanceBoundaryCalls: 1, acknowledged: input.stage === 'resume',
        }));
    } finally {
        bridge.stop();
    }
"#;

struct Fixture {
    directory: PathBuf,
    children: Vec<Child>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            directory: pij_testkit::fresh_dir("pij-native-cross-language"),
            children: Vec::new(),
        }
    }

    async fn host(&mut self) -> (usize, ProcIdentity) {
        let directory = self
            .directory
            .join(format!("owned-host-{}", self.children.len()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("copilot");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        #[cfg(target_os = "macos")]
        {
            let signed = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&executable)
                .output()
                .expect("macOS inert copied host requires /usr/bin/codesign");
            assert!(
                signed.status.success(),
                "{}",
                String::from_utf8_lossy(&signed.stderr)
            );
        }
        assert!(executable.is_absolute() && executable.as_os_str().len() > 16);
        let child = Command::new(&executable)
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let index = self.children.len();
        self.children.push(child);
        let proc_start = ProcLiveness::new().proc_start(pid).await.unwrap().unwrap();
        assert!(proc_start > 0);
        (index, ProcIdentity { pid, proc_start })
    }

    async fn kill_host(&mut self, index: usize, identity: ProcIdentity) {
        let child = &mut self.children[index];
        assert_eq!(child.id(), identity.pid);
        child.kill().expect("kill only the owned inert host");
        child
            .wait()
            .expect("reap owned host before death observation");
        assert_ne!(
            ProcLiveness::new().proc_start(identity.pid).await.unwrap(),
            Some(identity.proc_start),
            "real process adapter must observe absence or replacement of exact old tuple"
        );
    }

    async fn node(&mut self, runner: &str, module: &Path, input: &Value) -> Value {
        let stage = input["stage"].as_str().unwrap();
        let stdout_path = self.directory.join(format!("node-{stage}.stdout"));
        let stderr_path = self.directory.join(format!("node-{stage}.stderr"));
        let module_url = reqwest::Url::from_file_path(module).expect("absolute module path");
        let source = format!(
            "import {{ chooseRegistration, resolveRegistration, DaemonClient, FileJournal, NativeBridge }} from {};\n{runner}",
            serde_json::to_string(module_url.as_str()).unwrap(),
        );
        let child = Command::new("node")
            .args(["--input-type=module", "--eval", &source, &input.to_string()])
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout_path).unwrap())
            .stderr(std::fs::File::create(&stderr_path).unwrap())
            .spawn()
            .expect(
                "cross-language proof requires Node on PATH with fetch and AbortSignal.any support",
            );
        self.children.push(child);
        let child = self.children.last_mut().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "Node {stage} exceeded 15 seconds; stdout={} stderr={}",
                    std::fs::read_to_string(&stdout_path).unwrap(),
                    std::fs::read_to_string(&stderr_path).unwrap()
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let stdout = std::fs::read_to_string(&stdout_path).unwrap();
        let stderr = std::fs::read_to_string(&stderr_path).unwrap();
        assert!(
            status.success(),
            "Node {stage} failed: {status}\nstdout={stdout}\nstderr={stderr}"
        );
        println!("node-{stage}: {}", stdout.trim());
        if !stderr.is_empty() {
            eprintln!("node-{stage} stderr: {stderr}");
        }
        serde_json::from_str(&stdout).expect("one Node contract result")
    }
}

/// The daemon under test: real HTTP, SQLite registry, queue and spine, with
/// fake tmux panes for `panes`.
struct Daemon {
    addr: std::net::SocketAddr,
    registry: Arc<dyn Registry>,
    queue: Arc<dyn Queue>,
    server: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn serve(fixture: &Fixture, store: &FreshStore, panes: &[&str]) -> Self {
        let config = Config {
            adapters: Adapters {
                registry: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                liveness: AdapterChoice::Real,
                ..Adapters::default()
            },
            store_path: store.path(),
            ..Config::default()
        };
        let mut services = pij_daemon::build_services(&config, &fixture.directory.join("taps"))
            .await
            .unwrap();
        let mut tmux = FakeTmux::new();
        for pane in panes {
            tmux = tmux
                .with_pane(Pane {
                    id: (*pane).into(),
                    session: "s".into(),
                    window: "w".into(),
                    title: "cross-language native contract".into(),
                    cursor_x: Some(0),
                    cursor_y: Some(0),
                })
                .with_pane_process(
                    pane,
                    PaneProcess {
                        pid: std::process::id(),
                        cwd: fixture.directory.to_string_lossy().into_owned(),
                    },
                )
                .with_attached_tap(pane);
            tmux.arrange_clear_composer(pane);
        }
        let tmux = Arc::new(tmux);
        services.tmux = tmux.clone();
        services.interaction = Arc::new(pij_harnesses::InteractionGate::new(tmux));
        services.delivery = Arc::new(
            DeliveryService::new(
                services.registry.clone(),
                services.queue.clone(),
                services.transport.clone(),
                services.interaction.clone(),
                services.event_bus.clone(),
            )
            .unwrap(),
        );
        let (registry, queue) = (services.registry.clone(), services.queue.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(services, KEY.into()))
                .await
                .unwrap();
        });
        Self {
            addr,
            registry,
            queue,
            server,
        }
    }
}

/// The runtime under test: PIJ_NATIVE_RUNTIME_MODULE, else this checkout's.
fn runtime_module() -> PathBuf {
    let module = std::env::var_os("PIJ_NATIVE_RUNTIME_MODULE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.copilot/extensions/pij/store.mjs")
        });
    module.canonicalize().unwrap_or_else(|error| panic!(
        "runtime module {} unavailable: {error}; set PIJ_NATIVE_RUNTIME_MODULE to actual store.mjs (default applies after composition)",
        module.display()
    ))
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
#[ignore = "requires Node and actual runtime store.mjs; set PIJ_NATIVE_RUNTIME_MODULE then run cargo test --locked -p pij-daemon --test native_runtime_cold_resume -- --ignored --nocapture --test-threads=1"]
async fn actual_runtime_chooser_and_journal_resume_through_http_sqlite_without_reinjection() {
    let module = runtime_module();
    let mut fixture = Fixture::new();
    let (old_index, old_process) = fixture.host().await;
    let (_, new_process) = fixture.host().await;
    assert_ne!(old_process.pid, new_process.pid);
    let store = FreshStore::new();
    let daemon = Daemon::serve(&fixture, &store, &["%137-cross-old", "%137-cross-new"]).await;
    let (registry, queue, addr) = (daemon.registry.clone(), daemon.queue.clone(), daemon.addr);
    let mut input = json!({
        "stage": "initial", "addr": addr.to_string(), "key": KEY,
        "module": module, "stateDir": fixture.directory, "folder": fixture.directory,
        "session": SESSION, "body": BODY,
        "host": { "pid": old_process.pid, "proc_start": old_process.proc_start, "pane": "%137-cross-old" },
    });
    let initial = fixture.node(RUNNER, &module, &input).await;
    assert_eq!(initial["acknowledged"], false);
    let seat = SeatId::from(initial["registration"]["id"].as_str().unwrap());
    let job_id = JobId(initial["jobId"].as_u64().unwrap());
    let owner = registry.get(&seat).await.unwrap().unwrap();
    assert_eq!(owner.proc, Some(old_process));
    assert!(owner.native_extension_delivery);
    assert_eq!(owner.semantic_state, Some(SemanticState::Hold));
    let running = queue.claimed_delivery(job_id).await.unwrap().unwrap();
    let accepted = std::fs::read(initial["journalPath"].as_str().unwrap()).unwrap();

    input["previous"] = initial.clone();

    fixture.kill_host(old_index, old_process).await;
    assert_eq!(
        ProcLiveness::new()
            .proc_start(new_process.pid)
            .await
            .unwrap(),
        Some(new_process.proc_start)
    );
    // Model lease expiry with the real durable retry operation, never a new job.
    queue.retry(job_id, Duration::ZERO).await.unwrap();
    let pending = queue.peek(&[delivery_kind(&seat)]).await.unwrap().unwrap();
    assert_eq!(pending.0, job_id);
    assert_eq!(pending.1.payload, running.payload);
    input["stage"] = json!("resume");
    input["host"] = json!({ "pid": new_process.pid, "proc_start": new_process.proc_start, "pane": "%137-cross-new" });
    let resumed = fixture.node(RUNNER, &module, &input).await;
    assert_eq!(resumed["acknowledged"], true);
    assert_eq!(resumed["moduleDigest"], initial["moduleDigest"]);
    assert_eq!(resumed["journalDigest"], initial["journalDigest"]);
    assert_eq!(resumed["acceptanceBoundaryCalls"], 1);
    let rebound = registry.get(&seat).await.unwrap().unwrap();
    assert_eq!(rebound.proc, Some(new_process));
    assert_eq!(rebound.harness_session.as_deref(), Some(SESSION));
    assert_eq!(rebound.pane.as_deref(), Some("%137-cross-new"));
    assert!(rebound.native_extension_delivery && rebound.tombstoned_at.is_none());
    assert_eq!(rebound.semantic_state, Some(SemanticState::Hold));
    assert!(queue.claimed_delivery(job_id).await.unwrap().is_none());
    assert!(queue.peek(&[delivery_kind(&seat)]).await.unwrap().is_none());
    assert_eq!(
        std::fs::read(initial["journalPath"].as_str().unwrap()).unwrap(),
        accepted
    );
    println!(
        "cross-language contract: real HTTP/SQLite + actual selected runtime chooser/client/bridge/journal; self-reported Hold retained; old={old_process:?}, new={new_process:?}; one simulated native acceptance, zero reinjection; not real CLI/model acceptance"
    );
    daemon.server.abort();
}

// F154: the actual runtime registers a native seat, then answers a role request
// through `NativeBridge.send` (the `pij_send` tool's path) with in_reply_to.
const REPLY_RUNNER: &str = r#"
const input = JSON.parse(process.argv[1]);
const signal = AbortSignal.timeout(4000);
const client = new DaemonClient({ addr: input.addr, readKey: async () => input.key });
const roster = await client.request('/v1/seats?scope=local', undefined, signal);
const registration = await resolveRegistration(client, chooseRegistration({
    sessionId: input.session, host: input.host, folder: input.folder,
    seats: roster.seats, env: {},
}), signal);
if (input.stage === 'register') {
    console.log(JSON.stringify({ registration }));
} else {
    const native = {
        rpc: { eventLog: {
            async tail() { return { cursor: '0' }; },
            async read() { return { cursor: '0', events: [], hasMore: false, cursorStatus: 'ok' }; },
        } },
        on() { return () => {}; },
        async send() { throw new Error('the replying seat receives nothing here'); },
    };
    const journal = new FileJournal(input.stateDir, registration);
    const bridge = new NativeBridge({ registration, native, client, journal, report: () => {} });
    try {
        await bridge.register();
        const sent = await bridge.send({
            to: input.requester, message: 'answered', in_reply_to: input.request,
        });
        console.log(JSON.stringify({ registration, sent }));
    } finally {
        bridge.stop();
    }
}
"#;

#[tokio::test]
#[ignore = "requires Node and actual runtime store.mjs; set PIJ_NATIVE_RUNTIME_MODULE then run cargo test --locked -p pij-daemon --test native_runtime_cold_resume -- --ignored --nocapture --test-threads=1"]
async fn actual_runtime_native_reply_answers_the_role_request_it_names() {
    use pij_store::{ReplyMode, RequestState, RoleRequest, SqliteOrchestration};
    let module = runtime_module();
    let mut fixture = Fixture::new();
    let store = FreshStore::new();
    let daemon = Daemon::serve(&fixture, &store, &["%154-native", "%154-requester"]).await;
    let orchestration = SqliteOrchestration::new(pij_store::open(&store.path()).await.unwrap());
    let mut requester = pij_core::model::SeatDescriptor::new(
        "pij-requester",
        pij_core::model::Harness::Omp,
        fixture.directory.to_string_lossy(),
    );
    requester.pane = Some("%154-requester".into());
    daemon.registry.put(requester).await.unwrap();
    // The pane-bound seat binds by pane (pm-001's path); the paneless one by
    // its registered native session.
    for (label, pane) in [("pane", Some("%154-native")), ("paneless", None)] {
        let (_, host) = fixture.host().await;
        let mut input = json!({
            "stage": "register", "addr": daemon.addr.to_string(), "key": KEY,
            "stateDir": fixture.directory.join(label), "folder": fixture.directory,
            "session": format!("native-reply-{label}"), "requester": "pij-requester",
            "host": { "pid": host.pid, "proc_start": host.proc_start, "pane": pane },
        });
        std::fs::create_dir_all(fixture.directory.join(label)).unwrap();
        let registered = fixture.node(REPLY_RUNNER, &module, &input).await;
        let seat = registered["registration"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let request = format!("req-native-{label}");
        orchestration
            .record_role_request(&RoleRequest {
                id: request.clone(),
                project: "fleet".into(),
                role: "pm-001".into(),
                instance: String::new(),
                requester: "pij-requester".into(),
                body: "please answer".into(),
                in_reply_to: None,
                fyi: false,
                reply: ReplyMode::Wake,
                limit_ms: Some(3_600_000),
                state: RequestState::Delivered,
                holder: Some(seat.clone()),
                created_at: 1,
                delivered_at: Some(1),
                checkin_at: None,
                escalated_at: None,
                answered_at: None,
            })
            .await
            .unwrap();
        input["stage"] = json!("reply");
        input["request"] = json!(request);
        let replied = fixture.node(REPLY_RUNNER, &module, &input).await;
        assert_eq!(replied["registration"]["id"], seat.as_str(), "{label}");
        assert_eq!(replied["sent"]["ok"], true, "{label}: {replied}");
        let answered = orchestration.role_request(&request).await.unwrap().unwrap();
        assert!(answered.answered_at.is_some(), "{label}: {replied}");
    }
    daemon.server.abort();
}
