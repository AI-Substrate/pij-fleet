//! This entire module uses pre-plan-139 APIs: copy it to the baseline and add
//! `#[cfg(test)] mod governance_http_tests;` in events/mod.rs for a behavioral RED.
//! No live tmux/process adapters or daemon boot loop: a fresh SQLite store and
//! ephemeral loopback HTTP server exercise the real route composition only.

use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Pane, PaneProcess, SeatDescriptor, Seq};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};
use tokio::time::timeout;

struct Server(tokio::task::JoinHandle<()>);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Lines {
    response: reqwest::Response,
    pending: Vec<u8>,
}

impl Lines {
    async fn next(&mut self) -> Value {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = self.pending.drain(..=end).collect();
                    return serde_json::from_slice(&line).expect("NDJSON frame");
                }
                let chunk = self
                    .response
                    .chunk()
                    .await
                    .expect("read stream")
                    .expect("stream remains open");
                self.pending.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("live frame arrives without reconnect/replay")
    }
}

fn fixtures() -> (Value, Value) {
    let events = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-events.json"
    )))
    .unwrap();
    let routes = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-routes.json"
    )))
    .unwrap();
    (events, routes)
}

fn event_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn two_hello_confirmed_http_subscribers_see_adopt_and_report_before_message_control() {
    let (events, routes) = fixtures();
    let canonical = &event_case(&events, "seat-put")["decoded_payload"];
    let mut sender: SeatDescriptor = serde_json::from_value(canonical.clone()).unwrap();
    let proc = sender.proc.unwrap();
    let pane = sender.pane.clone().unwrap();
    let folder = sender.folder.clone();
    sender.id = routes["fixture_context"]["parent"].as_str().unwrap().into();
    sender.parent = None;
    sender.pane = None;
    sender.proc = None;
    let mut recipient = sender.clone();
    recipient.id = routes["fixture_context"]["outsider"]
        .as_str()
        .unwrap()
        .into();

    let fresh = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: fresh.path(),
        ..Config::default()
    };
    let signal_dir = std::path::PathBuf::from(fresh.path()).with_extension("signals");
    let mut services = crate::build_services(&config, &signal_dir).await.unwrap();
    services.registry.put(sender.clone()).await.unwrap();
    services.registry.put(recipient.clone()).await.unwrap();
    services.tmux = Arc::new(
        FakeTmux::new()
            .with_pane(Pane {
                id: pane.clone(),
                session: "fixture".to_string(),
                window: "@1".to_string(),
                title: "fixture".to_string(),
                cursor_x: None,
                cursor_y: None,
            })
            .with_pane_process(
                &pane,
                PaneProcess {
                    pid: proc.pid,
                    cwd: folder.clone(),
                },
            ),
    );
    services.liveness = Arc::new(FakeLiveness::new().with_proc(proc));
    let spine = services.spine.clone();
    let registry = services.registry.clone();
    let since = spine
        .tail(None, Seq(0))
        .await
        .unwrap()
        .last()
        .unwrap()
        .seq
        .unwrap();
    let machine = event_case(&events, "seat-put")["frame"]["machine"]
        .as_str()
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::http::router_with_config(
        services,
        crate::http::HttpConfig {
            local_key: "fixture-key".to_string(),
            peer_keys: Vec::new(),
            machine_alias: machine.to_string(),
        },
    );
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = reqwest::Client::new();
    let mut streams = Vec::new();
    for replay in [false, true] {
        let mut request = client
            .get(format!("http://{addr}/v1/events"))
            .bearer_auth("fixture-key");
        if replay {
            request = request.query(&[("since", json!({ machine: since.0 }).to_string())]);
        }
        let response = request.send().await.unwrap().error_for_status().unwrap();
        let mut stream = Lines {
            response,
            pending: Vec::new(),
        };
        let hello = stream.next().await;
        assert_eq!(hello["hello"], events["hello"]["hello"]);
        assert_eq!(hello["v"], events["hello"]["v"]);
        streams.push(stream);
    }

    let adopt_case = routes["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["path"] == "/v1/adopt")
        .unwrap();
    let mut adopt_request = adopt_case["cases"][0]["request"].clone();
    // u1's baseline witness excludes u2's new --role surface, but consumes the
    // canonical request rather than maintaining a second fixture by hand.
    let argv = adopt_request["argv"].as_array_mut().unwrap();
    let role = argv.iter().position(|arg| arg == "--role").unwrap();
    argv.drain(role..role + 2);
    let response: Value = client
        .post(format!("http://{addr}/v1/adopt"))
        .bearer_auth("fixture-key")
        .json(&adopt_request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["ok"], true, "adopt envelope: {response}");
    let adopted: SeatDescriptor = serde_json::from_value(response["data"].clone()).unwrap();
    let persisted = registry.get(&adopted.id).await.unwrap().unwrap();
    let card = &event_case(&events, "report-now")["decoded_payload"];
    let report: Value = client.post(format!("http://{addr}/v1/report")).bearer_auth("fixture-key")
        .json(&json!({ "caller": { "TMUX_PANE": pane, "cwd": folder }, "argv": ["report", "now", card["did"], card["next"]] }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(report["ok"], true, "report envelope: {report}");
    let send: Value = client.post(format!("http://{addr}/v1/send")).bearer_auth("fixture-key")
        .json(&json!({ "from": sender.id, "from_machine": null, "to": { "seat": recipient.id, "machine": null },
            "msg_id": "u1-positive-control", "body": card["did"], "in_reply_to": null }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(send["ok"], true, "send positive-control envelope: {send}");
    let control_kind = events["positive_control"]["emitted_to_both"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["kind"]
        .as_str()
        .unwrap();
    let durable = spine.tail(None, since).await.unwrap();
    assert!(
        durable.iter().any(|event| event.kind == control_kind),
        "positive control must actually publish"
    );

    let mut observed = Vec::new();
    for stream in &mut streams {
        let mut frames = Vec::new();
        loop {
            let frame = stream.next().await;
            let last = frame["event"]["kind"] == control_kind;
            frames.push(frame);
            if last {
                break;
            }
        }
        observed.push(frames);
    }
    assert_eq!(
        observed[0], observed[1],
        "both already-open streams observe the same facts"
    );
    let frames = &observed[0];
    assert_eq!(
        frames.len(),
        durable.len(),
        "baseline RED: message positive control arrived, but adopt/report writes were absent from both live streams"
    );
    for (frame, event) in frames.iter().zip(&durable) {
        assert_eq!(frame["type"], "event");
        assert_eq!(frame["machine"], machine);
        assert_eq!(frame["cursor"], event.seq.unwrap().0);
        assert!(frame["event"].get("seq").is_none());
        assert_eq!(frame["event"]["at"], event.at);
        assert!(
            event.at > 0,
            "new events cannot retain placeholder timestamps"
        );
        assert_eq!(frame["event"]["kind"], event.kind);
        assert_eq!(frame["event"]["payload"], event.payload);
    }
    for pair in frames.windows(2) {
        assert_eq!(
            pair[1]["cursor"].as_u64().unwrap(),
            pair[0]["cursor"].as_u64().unwrap() + 1
        );
    }
    let descriptor_frame = frames
        .iter()
        .find(|frame| {
            frame["event"]["kind"] == event_case(&events, "seat-put")["frame"]["event"]["kind"]
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<SeatDescriptor>(
            descriptor_frame["event"]["payload"].as_str().unwrap()
        )
        .unwrap(),
        persisted
    );
    let card_frame = frames
        .iter()
        .find(|frame| {
            frame["event"]["kind"] == event_case(&events, "report-now")["frame"]["event"]["kind"]
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(card_frame["event"]["payload"].as_str().unwrap()).unwrap(),
        *card
    );
}
