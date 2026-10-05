//! Real HTTP/store/process witnesses for daemon-owned detached jobs.
use std::path::PathBuf;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_daemon::{Daemon, boot, build_services};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

const OWNER: &str = "pij-bg-owner";
const OTHER: &str = "pij-bg-other";
const PARENT: &str = "pij-bg-parent";

struct Fixture {
    store: FreshStore,
    state: PathBuf,
    config: Config,
}

impl Fixture {
    async fn new() -> Self {
        let store = FreshStore::new();
        let state = PathBuf::from(store.path()).with_extension("bg-state");
        std::fs::create_dir_all(&state).unwrap();
        let config = Config {
            adapters: Adapters {
                registry: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                liveness: AdapterChoice::Real,
                ..Adapters::default()
            },
            store_path: store.path(),
            bind_addr: "127.0.0.1:0".into(),
            ..Config::default()
        };
        let services = build_services(&config, &state.join("pane-signals"))
            .await
            .unwrap();
        for id in [OWNER, OTHER, PARENT] {
            let mut seat = SeatDescriptor::new(id, Harness::Omp, state.to_str().unwrap());
            if id == OWNER {
                seat.parent = Some(SeatId::from(PARENT));
            }
            services.registry.put(seat).await.unwrap();
        }
        Self {
            store,
            state,
            config,
        }
    }

    async fn boot(&self) -> Daemon {
        boot(&self.config, self.state.clone()).await.unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _keep_store_alive = &self.store;
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

async fn post(daemon: &Daemon, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{}{path}", daemon.addr))
        .bearer_auth(&daemon.key.token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"undecodable": text}));
    (status, value)
}

async fn call(daemon: &Daemon, owner: &str, argv: Value) -> (u16, Value) {
    post(
        daemon,
        "/v1/bg",
        json!({"argv": argv, "caller": {"pijSessionId": owner}}),
    )
    .await
}

async fn wait_done(daemon: &Daemon, job: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = call(daemon, OWNER, json!(["bg", "list"])).await;
            if let Some(row) = value["data"]["jobs"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["job_id"] == job))
                && matches!(row["state"].as_str(), Some("done" | "killed" | "lost"))
                && row["notified"] == true
            {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("job reaches terminal state")
}

#[tokio::test]
async fn bg_create_list_tail_and_completion_are_owned_and_durable() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "contract",
            "--command",
            "printf 'BG-DONE\\n'; exit 7"
        ]),
    )
    .await;
    assert_eq!(status, 200, "bg create is a real route: {created}");
    let job = created["data"]["job"].as_str().unwrap();
    let row = wait_done(&daemon, job).await;
    assert_eq!(row["state"], "done");
    assert_eq!(row["exit_code"], 7);
    let (_, tail) = call(&daemon, OWNER, json!(["bg", "tail", job, "--lines", "1"])).await;
    assert_eq!(tail["data"]["lines"], json!(["BG-DONE"]));
    let (status, denied) = call(&daemon, OTHER, json!(["bg", "tail", job])).await;
    assert_eq!(status, 400, "unrelated seat refused: {denied}");
    let (_, own) = call(&daemon, OTHER, json!(["bg", "list"])).await;
    assert_eq!(own["data"]["jobs"], json!([]));
    let (_, parent) = call(&daemon, PARENT, json!(["bg", "list", "--all"])).await;
    assert_eq!(parent["data"]["jobs"][0]["job_id"], job);
    let (_, inbox) = post(
        &daemon,
        "/v1/shim/inbox",
        json!({"argv":["inbox"],"caller":{"pijSessionId":OWNER}}),
    )
    .await;
    assert!(
        inbox.to_string().contains("pij-bg"),
        "daemon virtual sender needs no seat: {inbox}"
    );
    assert!(inbox.to_string().contains("full log:"));
    assert!(inbox.to_string().contains("FAILED (exit 7)"));
    let (status, finished_kill) = call(&daemon, OWNER, json!(["bg", "kill", job])).await;
    assert_eq!(
        status, 400,
        "finished jobs must not claim a new kill: {finished_kill}"
    );
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_restart_preserves_runner_exit_code_and_delivers_completion() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "restart",
            "--command",
            "sleep 2; echo RESTART-DONE; exit 3"
        ]),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let job = created["data"]["job"].as_str().unwrap().to_owned();
    daemon.shutdown().await.unwrap();
    let daemon = fixture.boot().await;
    let row = wait_done(&daemon, &job).await;
    assert_eq!(row["state"], "done");
    assert_eq!(row["exit_code"], 3);
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_kill_requires_owner_or_recorded_parent_and_notifies() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, created) = call(
        &daemon,
        OWNER,
        json!(["bg", "create", "--title", "kill", "--command", "sleep 300"]),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let job = created["data"]["job"].as_str().unwrap();
    assert_eq!(
        call(&daemon, OTHER, json!(["bg", "kill", job])).await.0,
        400
    );
    assert_eq!(
        call(&daemon, PARENT, json!(["bg", "kill", job])).await.0,
        200
    );
    assert_eq!(wait_done(&daemon, job).await["state"], "killed");
    let (_, inbox) = post(
        &daemon,
        "/v1/shim/inbox",
        json!({"argv":["inbox"],"caller":{"pijSessionId":OWNER}}),
    )
    .await;
    assert!(inbox.to_string().contains("KILLED"), "{inbox}");
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_virtual_actor_cannot_be_addressed_or_impersonated() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, refusal) = post(
        &daemon,
        "/v1/shim/send",
        json!({"argv":["send","pij-bg","no"],"caller":{"pijSessionId":OWNER}}),
    )
    .await;
    assert_eq!(status, 400, "{refusal}");
    assert!(refusal.to_string().contains("daemon-owned"), "{refusal}");
    let (status, refusal) = post(
        &daemon,
        "/v1/bg",
        json!({"title":"forged","command":"true","owner":OTHER,"caller":{"pijSessionId":OWNER}}),
    )
    .await;
    assert_eq!(status, 400, "{refusal}");
    let (status, refusal) = post(
        &daemon,
        "/v1/send",
        json!({"from":"pij-bg","to":{"seat":OWNER},"body":"spoof","msg_id":"spoof"}),
    )
    .await;
    assert_eq!(status, 400, "{refusal}");
    assert!(refusal.to_string().contains("daemon-owned"));
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_rest_routes_require_bearer_and_derive_caller() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let client = reqwest::Client::new();
    for path in ["/v1/bg", "/v1/bg/unknown/tail"] {
        let response = client
            .get(format!("http://{}{path}", daemon.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }
    let (status, created) = post(
        &daemon,
        "/v1/bg",
        json!({"title":"rest","command":"sleep 300","caller":{"pijSessionId":OWNER}}),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let job = created["data"]["job"].as_str().unwrap();
    let response: Value = client
        .get(format!("http://{}/v1/bg?seat={OWNER}", daemon.addr))
        .bearer_auth(&daemon.key.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["data"]["jobs"][0]["job_id"], job);
    let response = client
        .get(format!(
            "http://{}/v1/bg/{job}/tail?seat={OTHER}&lines=2",
            daemon.addr
        ))
        .bearer_auth(&daemon.key.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        post(
            &daemon,
            &format!("/v1/bg/{job}/kill"),
            json!({"caller":{"pijSessionId":OTHER}})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        post(
            &daemon,
            &format!("/v1/bg/{job}/kill"),
            json!({"caller":{"pijSessionId":OWNER}})
        )
        .await
        .0,
        200
    );
    assert_eq!(wait_done(&daemon, job).await["state"], "killed");
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_restart_honors_persisted_kill_intent_before_signal() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (_, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "intent",
            "--command",
            "sleep 300"
        ]),
    )
    .await;
    let job = created["data"]["job"].as_str().unwrap().to_owned();
    daemon.shutdown().await.unwrap();
    let pool = pij_store::open(&fixture.store.path()).await.unwrap();
    let jobs = pij_store::background::SqliteBackground::new(pool);
    assert!(jobs.request_kill(&job).await.unwrap());
    let daemon = fixture.boot().await;
    assert_eq!(wait_done(&daemon, &job).await["state"], "killed");
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_runner_uses_owner_folder_private_log_and_daemon_identity_env() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (_, created) = call(&daemon, OWNER, json!(["bg", "create", "--title", "env", "--command", "pwd; printf '%s\\n' \"$PIJ_SESSION_ID\" \"$PIJ_BG_JOB\" \"$PIJ_BG_TITLE\" \"$PIJ_RS_ADDR\" \"$PIJ_RS_STATE_DIR\"; echo STDERR >&2"])).await;
    let job = created["data"]["job"].as_str().unwrap();
    assert_eq!(wait_done(&daemon, job).await["exit_code"], 0);
    let (_, tail) = call(&daemon, OWNER, json!(["bg", "tail", job])).await;
    assert_eq!(
        tail["data"]["lines"],
        json!([
            fixture.state.canonicalize().unwrap().to_str().unwrap(),
            OWNER,
            job,
            "env",
            daemon.addr.to_string(),
            fixture.state.to_str().unwrap(),
            "STDERR"
        ])
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(created["data"]["outPath"].as_str().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_relative_daemon_state_still_writes_receipt_beside_log() {
    struct RelativeState(PathBuf);
    impl Drop for RelativeState {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture::new().await;
    let unique = PathBuf::from(fixture.store.path())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let relative = RelativeState(PathBuf::from(format!(".bg-relative-{unique}")));
    let daemon = boot(&fixture.config, relative.0.clone()).await.unwrap();
    let (status, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "relative",
            "--command",
            "echo RELATIVE-DONE; exit 7"
        ]),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let row = wait_done(&daemon, created["data"]["job"].as_str().unwrap()).await;
    daemon.shutdown().await.unwrap();
    assert_eq!(
        row["state"], "done",
        "source exit must survive changing runner cwd: {row}"
    );
    assert_eq!(row["exit_code"], 7);
    assert!(PathBuf::from(created["data"]["outPath"].as_str().unwrap()).is_absolute());
}

async fn call_from(
    daemon: &Daemon,
    owner: &str,
    cwd: &std::path::Path,
    argv: Value,
) -> (u16, Value) {
    post(
        daemon,
        "/v1/bg",
        json!({"argv": argv, "caller": {"pijSessionId": owner, "cwd": cwd}}),
    )
    .await
}

#[tokio::test]
async fn bg_create_runs_in_caller_cwd_and_cwd_flag_overrides() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let caller = fixture.state.join("caller");
    let nested = caller.join("nested");
    let elsewhere = fixture.state.join("elsewhere");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    for (argv, expected) in [
        (
            json!(["bg", "create", "--title", "cwd", "--command", "pwd"]),
            &caller,
        ),
        (
            json!([
                "bg",
                "create",
                "--title",
                "cwd",
                "--cwd",
                "nested",
                "--command",
                "pwd"
            ]),
            &nested,
        ),
        (
            json!([
                "bg",
                "create",
                "--title",
                "cwd",
                "--cwd",
                elsewhere,
                "--command",
                "pwd"
            ]),
            &elsewhere,
        ),
    ] {
        let (status, created) = call_from(&daemon, OWNER, &caller, argv).await;
        assert_eq!(status, 200, "{created}");
        let job = created["data"]["job"].as_str().unwrap();
        wait_done(&daemon, job).await;
        let (_, tail) = call(&daemon, OWNER, json!(["bg", "tail", job])).await;
        assert_eq!(
            tail["data"]["lines"],
            json!([expected.canonicalize().unwrap().to_str().unwrap()])
        );
    }
    let (status, refusal) = call_from(
        &daemon,
        OWNER,
        &caller,
        json!([
            "bg",
            "create",
            "--title",
            "cwd",
            "--cwd",
            "missing",
            "--command",
            "pwd"
        ]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a missing directory is refused before launch: {refusal}"
    );
    assert!(refusal.to_string().contains("does not exist"), "{refusal}");
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn bg_timeout_kills_with_a_timeout_turn_and_list_shows_timing() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "slow",
            "--timeout",
            "1s",
            "--command",
            "echo STARTED; sleep 300"
        ]),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let job = created["data"]["job"].as_str().unwrap();
    let (_, listed) = call(&daemon, OWNER, json!(["bg", "list"])).await;
    let line = listed["data"]["line"].as_str().unwrap();
    assert!(
        line.contains("Running  running ") && line.ends_with("  slow"),
        "{line}"
    );
    let row = wait_done(&daemon, job).await;
    assert_eq!(row["state"], "killed");
    assert_eq!(row["timed_out"], true);
    let (_, listed) = call(&daemon, OWNER, json!(["bg", "list"])).await;
    let line = listed["data"]["line"].as_str().unwrap();
    assert!(
        line.contains(" TIMEOUT  took ") && line.ends_with("  slow"),
        "{line}"
    );
    let (_, inbox) = post(
        &daemon,
        "/v1/shim/inbox",
        json!({"argv":["inbox"],"caller":{"pijSessionId":OWNER}}),
    )
    .await;
    let inbox = inbox.to_string();
    assert!(
        inbox.contains("[pij bg] TIMEOUT — slow (killed after 1s)"),
        "{inbox}"
    );
    assert!(inbox.contains("tail: STARTED"), "{inbox}");
    let (status, refusal) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--title",
            "x",
            "--timeout",
            "0s",
            "--command",
            "true"
        ]),
    )
    .await;
    assert_eq!(status, 400, "{refusal}");
    daemon.shutdown().await.unwrap();
}

async fn emit(daemon: &Daemon, job: &str, token: &str, text: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/bg/{job}/emit", daemon.addr))
        .bearer_auth(token)
        .json(&json!({ "text": text, "data": {"n": text} }))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

/// Claim and acknowledge the owner's messages until one contains `needle`.
/// Delivery is serial per recipient: an unacknowledged claim holds the rest.
async fn inbox_until(daemon: &Daemon, needle: &str) -> String {
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let (_, inbox) = post(
            daemon,
            "/v1/shim/inbox",
            json!({"argv":["inbox"],"caller":{"pijSessionId":OWNER}}),
        )
        .await;
        for claim in inbox["data"].as_array().into_iter().flatten() {
            seen.push_str(claim["message"]["body"].as_str().unwrap_or_default());
            seen.push('\n');
            let (status, ack) = post(
                daemon,
                "/v1/shim/inbox/ack",
                json!({"caller":{"pijSessionId":OWNER},"job_id":claim["job_id"]}),
            )
            .await;
            assert_eq!(status, 200, "{ack}");
        }
        if seen.contains(needle) {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, listed) = call(daemon, OWNER, json!(["bg", "list"])).await;
    panic!("inbox never showed {needle:?}; seen: {seen}; jobs: {listed}")
}

#[tokio::test]
async fn bg_event_source_token_fires_across_a_restart_and_dies_at_kill() {
    let fixture = Fixture::new().await;
    let daemon = fixture.boot().await;
    let (status, created) = call(
        &daemon,
        OWNER,
        json!([
            "bg",
            "create",
            "--events",
            "--min-interval",
            "0",
            "--title",
            "source",
            "--command",
            "printf '%s' \"$PIJ_BG_TOKEN\" > \"$PIJ_RS_STATE_DIR/$PIJ_BG_JOB.token\"; sleep 300"
        ]),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let job = created["data"]["job"].as_str().unwrap().to_owned();
    let token_file = fixture.state.join(format!("{job}.token"));
    let token = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(token) = std::fs::read_to_string(&token_file)
                && token.len() == 64
            {
                return token;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the child sees PIJ_BG_TOKEN");
    assert!(
        !created.to_string().contains(&token),
        "the token never leaves the child"
    );
    for wrong in ["wrong", daemon.key.token.as_str()] {
        let (status, refusal) = emit(&daemon, &job, wrong, "x").await;
        assert_eq!(status, 401, "{refusal}");
    }
    let (status, fired) = emit(&daemon, &job, &token, "before").await;
    assert_eq!(
        (status, fired["data"]["seq"].clone()),
        (200, json!(1)),
        "{fired}"
    );
    inbox_until(&daemon, "1 new event from source").await;
    daemon.shutdown().await.unwrap();
    let daemon = fixture.boot().await;
    let (status, fired) = emit(&daemon, &job, &token, "after restart").await;
    assert_eq!(
        (status, fired["data"]["seq"].clone()),
        (200, json!(2)),
        "{fired}"
    );
    inbox_until(&daemon, "after restart").await;
    let (_, listed) = call(&daemon, OWNER, json!(["bg", "list"])).await;
    assert_eq!(listed["data"]["jobs"][0]["events"]["fired"], 2, "{listed}");
    assert!(
        listed["data"]["line"]
            .as_str()
            .unwrap()
            .contains("events: 2 fired, 0 pending"),
        "{listed}"
    );
    assert_eq!(
        call(&daemon, OWNER, json!(["bg", "kill", &job])).await.0,
        200
    );
    assert_eq!(wait_done(&daemon, &job).await["state"], "killed");
    inbox_until(&daemon, "STOPPED — source").await;
    let (status, refusal) = emit(&daemon, &job, &token, "too late").await;
    assert_eq!(status, 401, "the token dies with the job: {refusal}");
    daemon.shutdown().await.unwrap();
}
