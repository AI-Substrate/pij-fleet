//! Shipped all-real boot, with every native side effect confined to this fixture.
//! Run on Unix with tmux installed; a missing prerequisite is a failure, not a skip.
//! No process-global environment mutation and no production daemon/tmux probes.

#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use pij_testkit::fresh_dir;
use serde_json::Value;

const STARTUP: Duration = Duration::from_secs(30);
const COMMAND: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

struct Environment {
    root: PathBuf,
    path: OsString,
    address: SocketAddr,
    tmux: Option<String>,
    pane: Option<String>,
}

impl Environment {
    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.root.join("home"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("home/.claude"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PIJ_RS_STATE_DIR", self.root.join("state"))
            .env("PIJ_RS_ADDR", self.address.to_string())
            .env("PIJ_RS_BIND", self.address.to_string())
            .env("TMUX_TMPDIR", "/tmp")
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("LC_ALL", "C")
            .current_dir(&self.root)
            .stdin(Stdio::null());
        if let Some(tmux) = &self.tmux {
            command.env("TMUX", tmux);
        }
        if let Some(pane) = &self.pane {
            command.env("TMUX_PANE", pane);
        }
        command
    }

    fn cli(&self) -> Command {
        let mut command = self.command(env!("CARGO_BIN_EXE_pij-rs"));
        command
            .arg("--json")
            .arg("--state-dir")
            .arg(self.root.join("state"))
            .arg("--addr")
            .arg(self.address.to_string());
        command
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// File-backed output cannot deadlock a verbose child on a full pipe.
/// The child handle, not a registry PID, owns every fallback kill and reap.
struct Process {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Process {
    fn spawn(command: &mut Command, root: &Path, label: &str) -> Self {
        let stdout = root.join(format!("{label}.stdout"));
        let stderr = root.join(format!("{label}.stderr"));
        let child = command
            .stdout(File::create(&stdout).expect("stdout log"))
            .stderr(File::create(&stderr).expect("stderr log"))
            .spawn()
            .unwrap_or_else(|error| panic!("start {label}: {error}; command={command:?}"));
        Self {
            child,
            stdout,
            stderr,
        }
    }

    fn output(&self) -> String {
        fs::read_to_string(&self.stdout).expect("read stdout")
    }

    fn diagnostics(&self) -> String {
        format!(
            "stdout={}\nstderr={}",
            self.output(),
            fs::read_to_string(&self.stderr).unwrap_or_default()
        )
    }

    fn running(&mut self) {
        assert!(
            self.child.try_wait().expect("child status").is_none(),
            "child exited early: {}",
            self.diagnostics()
        );
    }

    async fn wait(&mut self, bound: Duration) -> ExitStatus {
        let deadline = Instant::now() + bound;
        loop {
            if let Some(status) = self.child.try_wait().expect("child status") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "child timed out: {}",
                self.diagnostics()
            );
            tokio::time::sleep(POLL).await;
        }
    }

    async fn wait_for(&mut self, predicate: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + STARTUP;
        loop {
            if predicate(&self.output()) {
                return;
            }
            self.running();
            assert!(
                Instant::now() < deadline,
                "expected output did not arrive: {}",
                self.diagnostics()
            );
            tokio::time::sleep(POLL).await;
        }
    }

    async fn interrupt(&mut self, environment: &Environment, label: &str) {
        self.running();
        // This is our unreaped Child's PID. It cannot be reused between this
        // check and signalling; no registry/process-name lookup chooses a target.
        let mut signal = environment.command("kill");
        signal.args(["-INT", &self.child.id().to_string()]);
        let mut signal = Self::spawn(&mut signal, &environment.root, label);
        assert!(
            signal.wait(COMMAND).await.success(),
            "signal owned child: {}",
            signal.diagnostics()
        );
        assert!(
            self.wait(COMMAND).await.success(),
            "graceful shutdown: {}",
            self.diagnostics()
        );
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Own a foreground (-D) tmux server, not a detached server borrowed from a user.
struct PrivateTmux {
    server: Process,
    label: String,
    path: OsString,
    root: PathBuf,
    socket: Option<PathBuf>,
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        // Even on panic, first ask OUR named server to close its panes. Bound the
        // control client; Process::drop then kills/reaps the foreground fallback.
        let mut command = Command::new("tmux");
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.root.join("home"))
            .env("TMUX_TMPDIR", "/tmp")
            .args(["-L", &self.label, "kill-server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut client) = command.spawn() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(client.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(POLL);
            }
            if !matches!(client.try_wait(), Ok(Some(_))) {
                let _ = client.kill();
            }
            let _ = client.wait();
        }
        if !matches!(self.server.child.try_wait(), Ok(Some(_))) {
            let _ = self.server.child.kill();
            let _ = self.server.child.wait();
        }
        if let Some(socket) = &self.socket {
            let _ = fs::remove_file(socket);
        }
    }
}

async fn run_json(environment: &Environment, args: &[&str], label: &str) -> Value {
    let mut command = environment.cli();
    command.args(args);
    let mut process = Process::spawn(&mut command, &environment.root, label);
    assert!(
        process.wait(COMMAND).await.success(),
        "CLI {label}: {}",
        process.diagnostics()
    );
    let value: Value = serde_json::from_str(process.output().trim()).expect("CLI JSON envelope");
    assert_eq!(value["ok"], true, "CLI {label}: {value}");
    assert_eq!(value["v"], 2);
    value
}

async fn run_command(environment: &Environment, command: &mut Command, label: &str) -> String {
    let mut process = Process::spawn(command, &environment.root, label);
    assert!(
        process.wait(COMMAND).await.success(),
        "{label}: {}",
        process.diagnostics()
    );
    process.output()
}

async fn run_refusal(
    environment: &Environment,
    args: &[&str],
    label: &str,
    code: Option<&str>,
) -> Value {
    let mut command = environment.cli();
    command.args(args);
    let mut process = Process::spawn(&mut command, &environment.root, label);
    assert!(
        !process.wait(COMMAND).await.success(),
        "CLI {label} unexpectedly succeeded: {}",
        process.diagnostics()
    );
    let value: Value = serde_json::from_str(process.output().trim()).expect("CLI refusal envelope");
    assert_eq!(value["v"], 2);
    assert_eq!(value["ok"], false, "CLI {label}: {value}");
    if let Some(code) = code {
        assert_eq!(value["details"]["code"], code, "CLI {label}: {value}");
    }
    value
}

// Same chunk-safe NDJSON primitive as daemon/tests/governance_composition.rs.
struct LiveFrames {
    response: reqwest::Response,
    pending: Vec<u8>,
}

impl LiveFrames {
    async fn open(environment: &Environment) -> Self {
        let key = fs::read_to_string(environment.root.join("state/daemon.key"))
            .expect("private bearer key");
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(COMMAND)
            .build()
            .unwrap();
        // No since query: this subscription cannot replay a missed mutation.
        let response = tokio::time::timeout(
            COMMAND,
            client
                .get(format!("http://{}/v1/events", environment.address))
                .bearer_auth(key.trim())
                .send(),
        )
        .await
        .expect("bounded live connect")
        .expect("live-only stream");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let mut frames = Self {
            response,
            pending: Vec::new(),
        };
        let hello = frames.next().await;
        assert_eq!(hello["hello"], true, "real live-only Hello: {hello}");
        assert_eq!(hello["v"], pij_core::wire::EVENT_VERSION);
        assert!(!hello["build"].as_str().unwrap().is_empty());
        frames
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(COMMAND, async {
            loop {
                if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                    let frame = serde_json::from_slice(&self.pending[..end])
                        .expect("complete live NDJSON frame");
                    self.pending.drain(..=end);
                    return frame;
                }
                let chunk = self
                    .response
                    .chunk()
                    .await
                    .expect("live stream chunk")
                    .expect("live stream remains open");
                self.pending.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("live frame before deadline")
    }

    async fn event(&mut self, kind: &str, seat: &str) -> Value {
        tokio::time::timeout(STARTUP, async {
            loop {
                let frame = self.next().await;
                if frame["event"]["kind"] == kind && frame["event"]["seat"] == seat {
                    return frame;
                }
            }
        })
        .await
        .expect("matching live event before deadline")
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_millis()
        .try_into()
        .expect("epoch milliseconds fit u64")
}

async fn exercise_governance_workflow(
    environment: &mut Environment,
    tail: &mut Process,
    parent: &str,
    parent_pane: &str,
) {
    let parent_role = run_json(environment, &["role", parent, "prime"], "parent-role").await;
    assert_eq!(parent_role["data"]["role"], "prime");
    assert_eq!(parent_role["data"]["assigned_by"], parent);
    let mut create = environment.command("tmux");
    create
        .args([
            "new-window",
            "-d",
            "-t",
            "governance-boot",
            "-n",
            "owned-worker",
            "-P",
            "-F",
            "#{pane_id}",
            "-c",
        ])
        .arg(&environment.root)
        .arg("exec /bin/sleep 300");
    let worker_pane = run_command(environment, &mut create, "tmux-worker")
        .await
        .trim()
        .to_string();
    assert!(worker_pane.starts_with('%'));
    assert_ne!(worker_pane, parent_pane);
    // The existing CLI replay tail already emitted the parent's event, proving
    // its Hello was consumed. Confirm the second live-only Hello BEFORE child
    // adoption: both subsequent frames must be live fanout, never replay rescue.
    tail.running();
    let mut live = LiveFrames::open(environment).await;
    environment.pane = Some(worker_pane.clone());
    let adopted = run_json(
        environment,
        &[
            "adopt",
            &worker_pane,
            "--harness",
            "omp",
            "--parent",
            parent,
            "--role",
            "worker",
        ],
        "adopt-worker",
    )
    .await;
    let adopt_observed_ms = epoch_ms();
    let worker = adopted["data"]["id"].as_str().unwrap();
    assert_ne!(worker, parent);
    assert_eq!(adopted["data"]["parent"], parent);
    assert_eq!(adopted["data"]["pane"], worker_pane);
    assert!(adopted["data"]["proc"]["proc_start"].as_u64().unwrap() > 0);
    assert!(
        adopted["data"]["harness_session"].is_null(),
        "sleep is not a native model session: {adopted}"
    );
    let live_adopt = live.event("seat.put", worker).await;
    let adopt_seq = live_adopt["cursor"].as_u64().unwrap();
    let descriptor: pij_core::model::SeatDescriptor =
        serde_json::from_str(live_adopt["event"]["payload"].as_str().unwrap())
            .expect("seat.put carries a complete raw SeatDescriptor");
    assert_eq!(descriptor.id.as_str(), worker);
    assert_eq!(descriptor.pane.as_deref(), Some(worker_pane.as_str()));
    assert_eq!(descriptor.parent.as_ref().unwrap().as_str(), parent);
    assert!(
        descriptor.machine.is_none(),
        "event descriptor is not an API machine-stamped join"
    );
    assert!(
        live_adopt["event"]["at"]
            .as_u64()
            .unwrap()
            .abs_diff(adopt_observed_ms)
            <= 1_000
    );
    tail.wait_for(|text| {
        frames(text)
            .iter()
            .any(|frame| frame["cursor"] == adopt_seq)
    })
    .await;
    let replay_adopt = frames(&tail.output());
    assert_eq!(
        replay_adopt
            .iter()
            .find(|frame| frame["cursor"] == adopt_seq)
            .unwrap(),
        &live_adopt
    );
    let child_report = run_json(
        environment,
        &[
            "report",
            "now",
            "Read private workflow",
            "Exercise native CLI contracts",
        ],
        "worker-live-report",
    )
    .await;
    let report_observed_ms = epoch_ms();
    let child_report_seq = child_report["data"]["seq"].as_u64().unwrap();
    assert!(child_report_seq > adopt_seq);
    let live_report = live.event("report.now", worker).await;
    assert_eq!(live_report["cursor"], child_report_seq);
    assert!(
        live_report["event"]["at"]
            .as_u64()
            .unwrap()
            .abs_diff(report_observed_ms)
            <= 1_000
    );
    tail.wait_for(|text| {
        frames(text)
            .iter()
            .any(|frame| frame["cursor"] == child_report_seq)
    })
    .await;
    let replay_report = frames(&tail.output());
    assert_eq!(
        replay_report
            .iter()
            .find(|frame| frame["cursor"] == child_report_seq)
            .unwrap(),
        &live_report
    );
    eprintln!(
        "private dual-stream worker={worker} adopt_seq={adopt_seq} report_seq={child_report_seq} adopt_at={} report_at={}",
        live_adopt["event"]["at"], live_report["event"]["at"]
    );
    drop(live);
    environment.pane = Some(parent_pane.to_string());
    let roster = run_json(environment, &["list"], "parent-worker-list").await;
    let seats = roster["data"]["seats"].as_array().unwrap();
    assert_eq!(seats.len(), 2, "only owned parent and worker: {roster}");
    let parent_row = seats.iter().find(|seat| seat["id"] == parent).unwrap();
    let worker_row = seats.iter().find(|seat| seat["id"] == worker).unwrap();
    assert_eq!(parent_row["role"], "prime");
    assert_eq!(worker_row["role"], "worker");
    assert_eq!(worker_row["parent"], parent);

    let repo = environment.root.join("repo");
    let worktrees = environment.root.join("worktrees");
    fs::create_dir_all(&worktrees).expect("private worktree root");
    let mut init = environment.command("git");
    init.args(["init", "-b", "main"]).arg(&repo);
    run_command(environment, &mut init, "git-init").await;
    let mut commit = environment.command("git");
    commit.arg("-C").arg(&repo).args([
        "-c",
        "user.name=Private workflow fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "Private workflow root",
    ]);
    run_command(environment, &mut commit, "git-initial-commit").await;
    let project_result = run_json(
        environment,
        &[
            "project",
            "create",
            "Private governance workflow",
            "--repo",
            repo.to_str().unwrap(),
        ],
        "project-create",
    )
    .await;
    let project = project_result["data"]["project"]["slug"].as_str().unwrap();
    assert_eq!(project_result["data"]["project"]["created_by"], parent);
    assert_eq!(
        project_result["data"]["project"]["repo"],
        repo.to_str().unwrap()
    );
    let project_seq = project_result["data"]["seq"].as_u64().unwrap();
    let stream_result = run_json(
        environment,
        &[
            "stream",
            "create",
            "--project",
            project,
            "--slug",
            "workflow",
            "--base",
            "main",
            "--root",
            worktrees.to_str().unwrap(),
        ],
        "stream-create",
    )
    .await;
    let stream = stream_result["data"]["stream"]["id"].as_str().unwrap();
    let worktree = PathBuf::from(
        stream_result["data"]["stream"]["worktree"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(stream_result["data"]["stream"]["project"], project);
    assert_eq!(stream_result["data"]["stream"]["state"], "created");
    assert!(stream_result["data"]["stream"]["ordinal"].as_u64().unwrap() > 0);
    assert!(
        worktree
            .canonicalize()
            .expect("real allocated worktree")
            .starts_with(&worktrees)
    );
    assert!(
        worktree.join(".git").is_file(),
        "actual linked worktree, not a fabricated registry row"
    );
    let stream_seq = stream_result["data"]["seq"].as_u64().unwrap();
    assert!(stream_seq > project_seq);
    let mut branch = environment.command("git");
    branch
        .arg("-C")
        .arg(&worktree)
        .args(["branch", "--show-current"]);
    assert_eq!(
        run_command(environment, &mut branch, "worktree-branch")
            .await
            .trim(),
        stream_result["data"]["stream"]["branch"].as_str().unwrap()
    );
    let fence_result = run_json(
        environment,
        &["fence", "set", stream, "--paths", "crates/cli/**"],
        "fence-set",
    )
    .await;
    let fence = fence_result["data"]["fence"]["id"].as_str().unwrap();
    let fences = run_json(
        environment,
        &["fence", "show", "--stream", stream],
        "fence-show",
    )
    .await;
    let declared = fences["data"]["fences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == fence)
        .unwrap();
    assert_eq!(declared["stream"], stream);
    assert_eq!(declared["paths"], serde_json::json!(["crates/cli/**"]));

    let packet_path = environment.root.join("workflow-packet.txt");
    let packet_bytes =
        b"Read and acknowledge these exact private fixture bytes. No live model is running.\n";
    fs::write(&packet_path, packet_bytes).expect("actual private packet file");
    let dispatched = run_json(
        environment,
        &[
            "dispatch",
            worker,
            "--packet",
            packet_path.to_str().unwrap(),
        ],
        "dispatch-packet",
    )
    .await;
    let dispatch = dispatched["data"]["dispatch"]["id"].as_str().unwrap();
    let msg_id = dispatched["data"]["dispatch"]["msg_id"].as_str().unwrap();
    assert_eq!(dispatched["data"]["dispatch"]["from"], parent);
    assert_eq!(dispatched["data"]["dispatch"]["to"], worker);
    assert_eq!(
        dispatched["data"]["dispatch"]["packet_path"],
        packet_path.to_str().unwrap()
    );
    environment.pane = Some(worker_pane.clone());
    let inbox = run_json(environment, &["inbox"], "packet-inbox-read").await;
    let message = inbox["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|msg| msg["msg_id"] == msg_id)
        .unwrap_or_else(|| panic!("dispatched packet missing from recipient inbox: {inbox}"));
    assert_eq!(message["from"], parent);
    assert_eq!(message["to"], worker);
    assert!(
        message["body"]
            .as_str()
            .unwrap()
            .contains(packet_path.to_str().unwrap())
    );
    let read_packet = fs::read(&packet_path).expect("recipient reads actual packet bytes");
    assert_eq!(read_packet, packet_bytes);
    let packet_sha = pij_testkit::fixtures::sha256_hex(&read_packet);
    assert_eq!(dispatched["data"]["dispatch"]["packet_sha256"], packet_sha);
    assert!(message["body"].as_str().unwrap().contains(&packet_sha));
    let empty = run_json(environment, &["inbox"], "packet-inbox-after-read").await;
    assert!(
        empty["data"].as_array().unwrap().is_empty(),
        "actual read is acknowledged: {empty}"
    );
    let acknowledged = run_json(
        environment,
        &["ack", dispatch, "--packet-sha", &packet_sha],
        "packet-sha-ack",
    )
    .await;
    assert_eq!(acknowledged["data"]["dispatch"]["id"], dispatch);
    assert_eq!(acknowledged["data"]["dispatch"]["state"], "acked");
    assert_eq!(acknowledged["data"]["dispatch"]["ack"]["seat"], worker);
    assert_eq!(
        acknowledged["data"]["dispatch"]["ack"]["packet_sha256"],
        packet_sha
    );
    let ack_seq = acknowledged["data"]["seq"].as_u64().unwrap();
    assert!(ack_seq > dispatched["data"]["seq"].as_u64().unwrap());

    environment.pane = Some(parent_pane.to_string());
    let attested = run_json(
        environment,
        &["attest", worker, "--plan-id", "139"],
        "attest-plan",
    )
    .await;
    assert_eq!(attested["data"]["attestation"]["seat"], worker);
    assert_eq!(attested["data"]["attestation"]["plan_id"], "139");
    assert_eq!(attested["data"]["attestation"]["attested_by"], parent);
    let assignment_result = run_json(
        environment,
        &[
            "task",
            "set",
            worker,
            "Exercise the shipped private workflow",
            "--project",
            project,
        ],
        "task-set",
    )
    .await;
    let assignment = assignment_result["data"]["task"]["id"].as_str().unwrap();
    assert_eq!(assignment_result["data"]["task"]["node_id"], worker);
    assert_eq!(assignment_result["data"]["task"]["opened_by"], parent);
    let node = run_json(environment, &["node", "show", worker], "worker-node").await;
    assert_eq!(node["data"]["node"]["id"], worker);
    assert_eq!(node["data"]["node"]["parent"], parent);
    assert_eq!(node["data"]["node"]["role"], "worker");
    assert_eq!(node["data"]["node"]["plan_id"], "139");
    assert!(
        node["data"]["node"]["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == assignment && row["project"] == project)
    );
    assert!(
        node["data"]["node"]["dispatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == dispatch && row["state"] == "acked")
    );
    let parent_node = run_json(environment, &["node", "show", parent], "parent-node").await;
    assert!(
        parent_node["data"]["node"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == worker)
    );

    // The real canary endpoint must refuse: no model reads this nonce packet.
    // This proves pending/no-model safety, NOT a positive runtime-model canary.
    let no_model = run_refusal(
        environment,
        &["canary", worker, "--wait=20"],
        "inert-canary",
        Some("E-RS-CANARY-PENDING"),
    )
    .await;
    let canary_dispatch = no_model["details"]["dispatch"].as_str().unwrap();
    assert!(!no_model["details"]["nonce"].as_str().unwrap().is_empty());
    environment.pane = Some(worker_pane.clone());
    let challenge = run_json(environment, &["inbox"], "unanswered-canary-inbox-read").await;
    assert!(
        challenge["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|msg| msg["msg_id"] == canary_dispatch)
    );
    // A scripted CLI transport read is deliberately not a packet SHA ACK.
    let pending = run_json(
        environment,
        &["node", "show", worker],
        "canary-remains-unverified",
    )
    .await;
    let canary = pending["data"]["node"]["dispatches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == canary_dispatch)
        .unwrap();
    assert_ne!(canary["state"], "acked");
    assert!(canary["canary"].is_null());
    eprintln!(
        "private workflow parent={parent} worker={worker} project={project} project_seq={project_seq} stream={stream} stream_seq={stream_seq} fence={fence} dispatch={dispatch} packet_sha={packet_sha} ack_seq={ack_seq} assignment={assignment} canary_refused={canary_dispatch}"
    );
    exercise_decisions_and_retirement(environment, tail, parent, parent_pane, worker, &worker_pane)
        .await;
}

async fn exercise_decisions_and_retirement(
    environment: &mut Environment,
    tail: &mut Process,
    parent: &str,
    parent_pane: &str,
    worker: &str,
    worker_pane: &str,
) {
    environment.pane = Some(worker_pane.to_string());
    let question_text = "Does this isolated CLI proof claim a live model?";
    let question = run_json(
        environment,
        &["report", "question", question_text],
        "question",
    )
    .await;
    let question_seq = question["data"]["seq"].as_u64().unwrap();
    let decision = question["data"]["decision"]["id"].as_str().unwrap();
    assert_eq!(question["data"]["decision"]["asked_by"], worker);
    assert_eq!(question["data"]["decision"]["parent"], parent);
    assert_eq!(question["data"]["decision"]["question_seq"], question_seq);
    tail.wait_for(|text| {
        frames(text).iter().any(|frame| {
            frame["cursor"] == question_seq && frame["event"]["kind"] == "decision.opened"
        })
    })
    .await;
    let question_frames = frames(&tail.output());
    let question_event = question_frames
        .iter()
        .find(|frame| frame["cursor"] == question_seq)
        .unwrap();
    let question_payload: Value =
        serde_json::from_str(question_event["event"]["payload"].as_str().unwrap()).unwrap();
    assert_eq!(question_payload["record"]["question_seq"], question_seq);

    environment.pane = Some(parent_pane.to_string());
    let decisions = run_json(environment, &["decisions"], "decisions-open").await;
    assert!(
        decisions["data"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["id"] == decision
                    && row["state"] == "open"
                    && row["question_seq"] == question_seq
            })
    );
    let answer_text = "No. These are inert panes; real-model canary proof remains separate.";
    let answer = run_json(environment, &["answer", decision, answer_text], "answer").await;
    assert_eq!(answer["data"]["decision"]["state"], "answered");
    assert_eq!(answer["data"]["decision"]["answered_by"], parent);
    assert_eq!(answer["data"]["decision"]["answer"], answer_text);
    let answer_msg = answer["data"]["decision"]["answer_msg_id"]
        .as_str()
        .unwrap();
    let answer_seq = answer["data"]["seq"].as_u64().unwrap();
    assert!(answer_seq > question_seq);

    environment.pane = Some(worker_pane.to_string());
    let inbox = run_json(environment, &["inbox"], "answer-inbox-read").await;
    let message = inbox["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|msg| msg["msg_id"] == answer_msg)
        .unwrap_or_else(|| panic!("answer message missing from actual recipient read: {inbox}"));
    assert_eq!(message["from"], parent);
    assert_eq!(message["to"], worker);
    assert!(message["body"].as_str().unwrap().contains(answer_text));
    let empty = run_json(environment, &["inbox"], "answer-inbox-after-read").await;
    assert!(
        empty["data"].as_array().unwrap().is_empty(),
        "recipient read ACK: {empty}"
    );
    let done = run_json(environment, &["report", "state", "done"], "worker-done").await;
    let done_seq = done["data"]["seq"].as_u64().unwrap();
    environment.pane = Some(parent_pane.to_string());
    let unverified = run_json(environment, &["anomalies"], "done-unverified").await;
    assert!(
        unverified["data"]["anomalies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["kind"] == "unverified-done"
                    && row["nodeId"] == worker
                    && row["evidence"]
                        .as_array()
                        .unwrap()
                        .contains(&Value::from(done_seq))
            }),
        "fresh done must require verification: {unverified}"
    );
    let verified = run_json(environment, &["report", "verify", worker], "parent-verify").await;
    assert_eq!(verified["data"]["done_seq"], done_seq);
    assert_eq!(verified["data"]["verified_by"], parent);
    let verify_seq = verified["data"]["seq"].as_u64().unwrap();
    assert!(verify_seq > done_seq);
    let clear = run_json(environment, &["anomalies"], "done-verified").await;
    assert!(
        !clear["data"]["anomalies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row["kind"] == "unverified-done" && row["nodeId"] == worker }),
        "verification clears only its current done claim: {clear}"
    );
    environment.pane = Some(worker_pane.to_string());
    let done_again = run_json(
        environment,
        &["report", "state", "done"],
        "worker-done-again",
    )
    .await;
    let new_done_seq = done_again["data"]["seq"].as_u64().unwrap();
    assert!(new_done_seq > verify_seq);
    environment.pane = Some(parent_pane.to_string());
    let reopened = run_json(environment, &["anomalies"], "done-reopened").await;
    assert!(
        reopened["data"]["anomalies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["kind"] == "unverified-done"
                    && row["nodeId"] == worker
                    && row["evidence"]
                        .as_array()
                        .unwrap()
                        .contains(&Value::from(new_done_seq))
            }),
        "a new done cannot borrow the previous verification: {reopened}"
    );

    // Both panes are still live. Compare actual cursors, not a sleeping tail,
    // to prove that dry-run neither appends an event nor removes either seat.
    let before = run_json(
        environment,
        &["spine", "events", "--since", "0"],
        "reap-before",
    )
    .await;
    let cursor = before["data"]["cursor"].as_u64().unwrap();
    let dry = run_json(environment, &["reap", "--dry-run"], "reap-dry-run").await;
    assert_eq!(dry["data"]["dry_run"], true);
    assert_eq!(dry["data"]["before"], 2);
    assert_eq!(dry["data"]["after"], 2);
    assert!(dry["data"]["reaped"].as_array().unwrap().is_empty());
    assert!(dry["data"]["candidates"].as_array().unwrap().is_empty());
    let after = run_json(
        environment,
        &["spine", "events", "--since", &cursor.to_string()],
        "reap-after",
    )
    .await;
    assert_eq!(after["data"]["cursor"], cursor);
    assert!(after["data"]["events"].as_array().unwrap().is_empty());
    let before_close = run_json(environment, &["list"], "list-before-close").await;
    assert_eq!(before_close["data"]["seats"].as_array().unwrap().len(), 2);

    let closed = run_json(environment, &["close", worker], "parent-close-worker").await;
    let tombstone_seq = closed["data"]["seq"].as_u64().unwrap();
    assert_eq!(closed["data"]["seat"], worker);
    assert!(tombstone_seq > new_done_seq);
    tail.wait_for(|text| {
        frames(text).iter().any(|frame| {
            frame["cursor"] == tombstone_seq
                && frame["event"]["kind"] == "seat.tombstone"
                && frame["event"]["seat"] == worker
        })
    })
    .await;
    let live = run_json(environment, &["list"], "list-after-close").await;
    let live_seats = live["data"]["seats"].as_array().unwrap();
    assert_eq!(live_seats.len(), 1, "closed seat excluded: {live}");
    assert_eq!(live_seats[0]["id"], parent);
    // Retirement must not kill the pane, and a retired caller must receive
    // the original tombstone receipt rather than silently acquiring a new seat.
    let mut pane_probe = environment.command("tmux");
    pane_probe.args(["display-message", "-p", "-t", worker_pane, "#{pane_id}"]);
    assert_eq!(
        run_command(environment, &mut pane_probe, "pane-survives-close")
            .await
            .trim(),
        worker_pane
    );
    environment.pane = Some(worker_pane.to_string());
    let retired = run_refusal(
        environment,
        &["report", "now", "retired", "must refuse"],
        "retired-self",
        None,
    )
    .await;
    assert_eq!(retired["details"]["seat"], worker);
    assert_eq!(retired["details"]["tombstone_seq"], tombstone_seq);
    assert!(
        retired["meta"]
            .as_str()
            .unwrap()
            .contains(&tombstone_seq.to_string())
    );
    environment.pane = Some(parent_pane.to_string());
    eprintln!(
        "private lifecycle parent={parent} worker={worker} decision={decision} question_seq={question_seq} answer_seq={answer_seq} done_seq={done_seq} verify_seq={verify_seq} reopened_done_seq={new_done_seq} tombstone_seq={tombstone_seq}"
    );
}

fn frames(text: &str) -> Vec<Value> {
    // The writer can be between bytes of its final line. Only complete NDJSON
    // lines are evidence; malformed COMPLETE lines still fail the test.
    text.split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).expect("complete event frame"))
        .collect()
}

#[tokio::test]
async fn shipped_all_real_daemon_boots_and_streams_on_fully_private_resources() {
    let root = fresh_dir("p139-boot")
        .canonicalize()
        .expect("native fixture directory");
    for path in ["home/.claude", "home/.config", "state"] {
        fs::create_dir_all(root.join(path)).expect("private directory");
    }
    // Keep the reservation until immediately before spawning the daemon. A bind
    // race fails with its real startup log; it never falls back to port 7461.
    let reservation = TcpListener::bind("127.0.0.1:0").expect("reserve private port");
    let address = reservation.local_addr().unwrap();
    assert_ne!(address.port(), 7461);
    let mut environment = Environment {
        root,
        path: std::env::var_os("PATH").expect("PATH for installed tmux and system tools"),
        address,
        tmux: None,
        pane: None,
    };
    let label = environment
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut start = environment.command("tmux");
    start.args(["-L", &label, "-f", "/dev/null", "-D"]);
    let mut tmux = PrivateTmux {
        server: Process::spawn(&mut start, &environment.root, "tmux-server"),
        label,
        path: environment.path.clone(),
        root: environment.root.clone(),
        socket: None,
    };
    // show-options does not auto-start a server. Wait until OUR foreground
    // server is responsive before issuing new-session, which otherwise can.
    let deadline = Instant::now() + STARTUP;
    loop {
        tmux.server.running();
        let mut probe = environment.command("tmux");
        probe.args(["-L", &tmux.label, "show-options", "-s", "exit-empty"]);
        let mut probe = Process::spawn(&mut probe, &environment.root, "tmux-ready");
        if probe.wait(COMMAND).await.success() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "private tmux readiness: {}",
            tmux.server.diagnostics()
        );
        tokio::time::sleep(POLL).await;
    }
    let mut create = environment.command("tmux");
    create
        .args([
            "-L",
            &tmux.label,
            "new-session",
            "-d",
            "-s",
            "governance-boot",
            "-n",
            "owned-pane",
            "-P",
            "-F",
            "#{socket_path}|#{pid}|#{pane_id}",
            "-c",
        ])
        .arg(&environment.root)
        .arg("exec /bin/sleep 300");
    let mut create = Process::spawn(&mut create, &environment.root, "tmux-session");
    assert!(
        create.wait(COMMAND).await.success(),
        "create private pane: {}",
        create.diagnostics()
    );
    let identity = create.output();
    let parts: Vec<_> = identity.trim().split('|').collect();
    assert_eq!(
        parts.len(),
        3,
        "actual private socket/server/pane: {identity}"
    );
    let socket = PathBuf::from(parts[0]);
    assert!(socket.exists(), "actual private tmux socket exists");
    tmux.socket = Some(socket.clone());
    assert_eq!(
        parts[1].parse::<u32>().unwrap(),
        tmux.server.child.id(),
        "-D server is the child this fixture owns"
    );
    let pane = parts[2].to_string();
    environment.tmux = Some(format!("{},{},0", socket.display(), parts[1]));
    environment.pane = Some(pane.clone());
    // No -L/-S here: prove the inherited TMUX used by daemon AND clients selects
    // only our private server. We never enumerate the user's default server.
    let mut probe = environment.command("tmux");
    probe.args(["list-panes", "-a", "-F", "#{pane_id}"]);
    let mut probe = Process::spawn(&mut probe, &environment.root, "tmux-environment");
    assert!(
        probe.wait(COMMAND).await.success(),
        "TMUX selection: {}",
        probe.diagnostics()
    );
    assert_eq!(probe.output().trim(), pane);

    let mut boot = environment.command(env!("CARGO_BIN_EXE_pij-rs"));
    boot.arg("--state-dir")
        .arg(environment.root.join("state"))
        .args(["daemon", "--bind", &address.to_string()]);
    drop(reservation);
    let mut daemon = Process::spawn(&mut boot, &environment.root, "daemon");
    daemon
        .wait_for(|text| {
            text.contains(&format!("pij-rs daemon: listening on {address}"))
                && text.contains("offline=false")
        })
        .await;
    TcpStream::connect_timeout(&address, COMMAND).expect("announced private listener accepts TCP");
    let ping = run_json(&environment, &["ping"], "ping").await;
    assert_eq!(ping["data"]["status"], "healthy");
    assert_eq!(ping["data"]["offline"], false);
    assert!(
        environment.root.join("state/pij.sqlite").is_file(),
        "real store exists"
    );
    assert!(
        environment.root.join("state/daemon.key").is_file(),
        "private authentication key exists"
    );

    let adopted = run_json(&environment, &["adopt", &pane, "--harness", "omp"], "adopt").await;
    let seat = adopted["data"]["id"].as_str().expect("adopted seat");
    assert_eq!(adopted["data"]["pane"], pane);
    assert_eq!(
        adopted["data"]["folder"],
        environment.root.to_string_lossy().as_ref()
    );
    assert!(
        adopted["data"]["proc"]["proc_start"].as_u64().unwrap() > 0,
        "real process corroboration"
    );
    let machine = ping["data"]["machine"].as_str().unwrap();
    let mut tail_command = environment.cli();
    tail_command.args(["tail", "--since", &format!("{machine}=0")]);
    let mut tail = Process::spawn(&mut tail_command, &environment.root, "events");
    // CLI tail validates and consumes the real Hello before printing any frame.
    tail.wait_for(|text| {
        frames(text)
            .iter()
            .any(|frame| frame["event"]["kind"] == "seat.put" && frame["event"]["seat"] == seat)
    })
    .await;
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-events.json"
    )))
    .unwrap();
    let card = &contract["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "report-now")
        .unwrap()["decoded_payload"];
    let report = run_json(
        &environment,
        &[
            "report",
            "now",
            card["did"].as_str().unwrap(),
            card["next"].as_str().unwrap(),
        ],
        "report",
    )
    .await;
    let seq = report["data"]["seq"].as_u64().unwrap();
    tail.wait_for(|text| {
        frames(text)
            .iter()
            .any(|frame| frame["cursor"] == seq && frame["event"]["kind"] == "report.now")
    })
    .await;
    let observed = frames(&tail.output());
    let frame = observed
        .iter()
        .find(|frame| frame["cursor"] == seq)
        .unwrap();
    assert_eq!(frame["machine"], machine);
    assert_eq!(frame["event"]["seat"], seat);
    assert!(frame["event"]["at"].as_u64().unwrap() > 0);
    assert_eq!(
        serde_json::from_str::<Value>(frame["event"]["payload"].as_str().unwrap()).unwrap(),
        *card
    );
    let roster = run_json(&environment, &["list"], "list").await;
    let seats = roster["data"]["seats"].as_array().unwrap();
    assert_eq!(
        seats.len(),
        1,
        "fresh private daemon must not acquire foreign seats: {roster}"
    );
    assert_eq!(seats[0]["pane"], pane);
    exercise_governance_workflow(&mut environment, &mut tail, seat, &pane).await;

    // Close the event client first so graceful HTTP shutdown can finish, then
    // the daemon (which drains admitted publications), then our private server.
    tail.interrupt(&environment, "interrupt-events").await;
    daemon.interrupt(&environment, "interrupt-daemon").await;
    let mut stop = environment.command("tmux");
    stop.args(["-L", &tmux.label, "kill-server"]);
    let mut stop = Process::spawn(&mut stop, &environment.root, "stop-tmux");
    assert!(
        stop.wait(COMMAND).await.success(),
        "private server teardown: {}",
        stop.diagnostics()
    );
    assert!(
        tmux.server.wait(COMMAND).await.success(),
        "foreground tmux exits cleanly"
    );
    // tmux may leave the pathname after a clean exit; this fixture owns it.
    if let Err(error) = fs::remove_file(&socket) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "unlink owned socket"
        );
    }
    assert!(!socket.exists(), "owned tmux socket was removed");
}
