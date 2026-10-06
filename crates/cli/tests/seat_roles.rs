//! Plan 166: a governor stamps roles from above on the placement call.

use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{ProcIdentity, SeatId, Seq};
use pij_core::ports::Spine;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use pij_testkit::fresh_dir;
use serde_json::Value;

const KEY: &str = "seat-roles-key";
const GOVERNOR: ProcIdentity = ProcIdentity {
    pid: 6161,
    proc_start: 20261006100000,
};
const HAND: ProcIdentity = ProcIdentity {
    pid: 6262,
    proc_start: 20261006100001,
};

struct Daemon {
    dir: std::path::PathBuf,
    addr: String,
    spine: Arc<dyn Spine>,
    tmux: Arc<FakeTmux>,
    server: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn start(label: &str) -> Self {
        let dir = fresh_dir(label);
        std::fs::write(dir.join("daemon.key"), KEY).expect("write client key");
        let config = Config {
            store_path: dir.join("pij.sqlite").display().to_string(),
            adapters: Adapters {
                registry: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                ..Adapters::default()
            },
            ..Config::default()
        };
        let mut services = pij_daemon::build_services(&config, &dir.join("pane-signals"))
            .await
            .expect("coherent real registry and spine");
        let tmux = Arc::new(FakeTmux::new());
        services.tmux = tmux.clone();
        services.liveness = Arc::new(FakeLiveness::new().with_proc(GOVERNOR).with_proc(HAND));
        let spine = services.spine.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind server");
        let addr = listener.local_addr().expect("server address").to_string();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router_with_config(
                    services,
                    HttpConfig {
                        local_key: KEY.to_string(),
                        peer_keys: Vec::new(),
                        machine_alias: "test-machine".to_string(),
                    },
                ),
            )
            .await
            .expect("serve router");
        });
        Self {
            dir,
            addr,
            spine,
            tmux,
            server,
        }
    }

    fn run(&self, caller: Option<&str>, args: &[&str]) -> (bool, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pij-rs"));
        command
            .args([
                "--state-dir",
                self.dir.to_str().expect("UTF-8 state dir"),
                "--addr",
                &self.addr,
                "--json",
            ])
            .args(args)
            .env_remove("PIJ_SESSION_ID")
            .env_remove("TMUX_PANE")
            .env_remove("COPILOT_AGENT_SESSION_ID")
            .env_remove("HARNESS_SESSION_ID")
            .env_remove("PIJ_PARENT_ID");
        if let Some(caller) = caller {
            command.env("PIJ_SESSION_ID", caller);
        }
        let output = command.output().expect("run shipped CLI");
        let json = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "CLI JSON: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.success(), json)
    }

    fn register(&self, id: &str, proc: ProcIdentity) {
        let (ok, json) = self.run(
            None,
            &[
                "register",
                id,
                "--harness",
                "claude",
                "--folder",
                "/abs/tree",
                "--pid",
                &proc.pid.to_string(),
                "--proc-start",
                &proc.proc_start.to_string(),
            ],
        );
        assert!(ok, "register {id}: {json}");
    }

    fn seat(&self, id: &str) -> Value {
        let (ok, json) = self.run(None, &["list"]);
        assert!(ok, "list: {json}");
        json["data"]["seats"]
            .as_array()
            .expect("seats")
            .iter()
            .find(|seat| seat["id"] == id)
            .unwrap_or_else(|| panic!("{id} in /v1/seats: {json}"))
            .clone()
    }

    async fn role_sets(&self, id: &str) -> Vec<Value> {
        self.spine
            .tail(Some(&SeatId::from(id)), Seq(0))
            .await
            .expect("spine tail")
            .into_iter()
            .filter(|event| event.kind == "role-set")
            .map(|event| serde_json::from_str(&event.payload).expect("role-set payload"))
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn spawn_args<'a>(id: &'a str, role: &'a str) -> Vec<&'a str> {
    vec![
        "spawn",
        "--id",
        id,
        "--harness",
        "copilot",
        "--cwd",
        "/abs/tree",
        "--session",
        "fleet",
        "--no-wait",
        "--role",
        role,
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_role_is_stamped_from_above_with_one_role_set() {
    let daemon = Daemon::start("pij-166-spawn-role").await;
    daemon.register("pij-gov", GOVERNOR);

    let (ok, json) = daemon.run(Some("pij-gov"), &spawn_args("pij-kid", "worker"));
    assert!(ok, "spawn --role worker: {json}");

    let seat = daemon.seat("pij-kid");
    assert_eq!(seat["role"], "worker", "{seat}");
    assert_eq!(seat["parent"], "pij-gov", "{seat}");
    let events = daemon.role_sets("pij-kid").await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["record"]["role"], "worker");
    assert_eq!(events[0]["record"]["assigned_by"], "pij-gov");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_without_role_records_nothing_and_foreign_vocabulary_launches_nothing() {
    let daemon = Daemon::start("pij-166-spawn-vocab").await;
    daemon.register("pij-gov", GOVERNOR);

    let (ok, json) = daemon.run(
        Some("pij-gov"),
        &[
            "spawn",
            "--id",
            "pij-plain",
            "--harness",
            "copilot",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--no-wait",
        ],
    );
    assert!(ok, "plain spawn: {json}");
    assert!(daemon.seat("pij-plain")["role"].is_null());
    assert!(daemon.role_sets("pij-plain").await.is_empty());
    let launched = daemon.tmux.calls().len();

    let (ok, json) = daemon.run(Some("pij-gov"), &spawn_args("pij-coder", "coder"));
    assert!(!ok, "coder is not a seat role: {json}");
    let message = json["meta"].as_str().unwrap_or_default();
    assert!(message.contains("pm, worker, pa"), "{json}");
    assert_eq!(daemon.tmux.calls().len(), launched, "refused before launch");

    let (ok, json) = daemon.run(
        Some("pij-gov"),
        &[
            "spawn",
            "--id",
            "pij-foreign",
            "--harness",
            "copilot",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--no-wait",
            "--parent",
            "pij-someone-else",
            "--role",
            "worker",
        ],
    );
    assert!(!ok, "a role from above names the spawner as parent: {json}");
    assert_eq!(json["details"]["code"], "E-RS-OWNERSHIP", "{json}");
    assert_eq!(daemon.tmux.calls().len(), launched, "refused before launch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_takes_a_hand_started_seat_and_stamps_its_role() {
    let daemon = Daemon::start("pij-166-link").await;
    daemon.register("pij-gov", GOVERNOR);
    daemon.register("pij-hand", HAND);

    let (ok, json) = daemon.run(Some("pij-gov"), &["link", "pij-hand", "--role", "worker"]);
    assert!(ok, "link --role worker: {json}");
    let seat = daemon.seat("pij-hand");
    assert_eq!(seat["parent"], "pij-gov", "{seat}");
    assert_eq!(seat["role"], "worker", "{seat}");
    let events = daemon.role_sets("pij-hand").await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["record"]["assigned_by"], "pij-gov");

    // The seat is now owned: the child cannot capture its own governor.
    let (ok, json) = daemon.run(Some("pij-hand"), &["link", "pij-gov", "--role", "pm"]);
    assert!(!ok, "cycle: {json}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_role_outside_the_vocabulary_is_a_json_refusal_naming_the_set() {
    let daemon = Daemon::start("pij-166-register-vocab").await;
    let (ok, json) = daemon.run(
        None,
        &[
            "register",
            "pij-coder",
            "--harness",
            "claude",
            "--folder",
            "/abs/tree",
            "--pid",
            &GOVERNOR.pid.to_string(),
            "--proc-start",
            &GOVERNOR.proc_start.to_string(),
            "--role",
            "coder",
        ],
    );
    assert!(!ok, "{json}");
    assert_eq!(json["error"], "refused", "{json}");
    assert_eq!(json["details"]["code"], "E-RS-ARG", "{json}");
    let message = json["meta"].as_str().unwrap_or_default();
    assert!(message.contains("prime, pm, worker, pa"), "{json}");
    let (ok, json) = daemon.run(None, &["list"]);
    assert!(ok, "list: {json}");
    assert!(
        json["data"]["seats"].as_array().expect("seats").is_empty(),
        "a refused role admits no seat: {json}"
    );
}
