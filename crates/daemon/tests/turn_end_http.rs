//! `POST /v1/turn-end` (plan 226), against a real daemon over HTTP.
//!
//! The block's commands must meet exactly what `pij send`, `pij output` and
//! `pij requests close` meet, so these drive the real routes and read the
//! results back the way a recipient does: from its inbox.

use std::net::SocketAddr;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, Harness, SeatDescriptor};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

const KEY: &str = "test-local-key";
const PANE: &str = "%226";
const SENDER: &str = "pij-turn-sender";
const RECIPIENT: &str = "pij-turn-recipient";
const RECIPIENT_PANE: &str = "%227";

async fn daemon() -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
    let mut sender = SeatDescriptor::new(SENDER, Harness::Omp, "/abs/tree");
    sender.pane = Some(PANE.to_string());
    let mut recipient = SeatDescriptor::new(RECIPIENT, Harness::Omp, "/abs/tree");
    recipient.pane = Some(RECIPIENT_PANE.to_string());
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
        std::path::Path::new("/tmp/pij-test-pane-signals-turn-end"),
    )
    .await
    .expect("services");
    // Pull-only: no fabricated socket delivery consumes the message first.
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
    for seat in [sender, recipient] {
        services.registry.put(seat).await.expect("seed seat");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = router_with_config(
        services,
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local(KEY.to_string()),
            machine_alias: "workstation".to_string(),
        },
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, server, store)
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .expect("post")
}

/// Hand `text` to the endpoint as the sender's harness would.
async fn turn_end(addr: SocketAddr, text: &str) -> Value {
    let response = post(
        addr,
        "/v1/turn-end",
        json!({ "text": text, "caller": { "TMUX_PANE": PANE, "cwd": "/abs/tree" }, "via": "hook:omp" }),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let envelope: Envelope<Value> = response.json().await.expect("envelope");
    assert!(
        envelope.ok,
        "the endpoint answers ok whenever it processed the text"
    );
    envelope.data.expect("data")
}

/// What the recipient would read: every queued message, oldest first,
/// claimed and acknowledged until none is left.
async fn inbox(addr: SocketAddr) -> Vec<Value> {
    let caller = json!({ "tmuxPane": RECIPIENT_PANE });
    let mut messages = Vec::new();
    for _ in 0..32 {
        let claimed: Envelope<Vec<Value>> = post(
            addr,
            "/v1/shim/inbox",
            json!({ "argv": ["inbox"], "caller": caller }),
        )
        .await
        .json()
        .await
        .expect("claim envelope");
        let claims = claimed.data.expect("claims");
        if claims.is_empty() {
            return messages;
        }
        for claim in claims {
            let acked = post(
                addr,
                "/v1/shim/inbox/ack",
                json!({ "caller": caller, "job_id": claim["job_id"] }),
            )
            .await;
            assert_eq!(acked.status(), reqwest::StatusCode::OK);
            messages.push(claim["message"].clone());
        }
    }
    panic!("the inbox never emptied");
}

fn statuses(result: &Value) -> Vec<&str> {
    result["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| line["status"].as_str().expect("status"))
        .collect()
}

fn block(lines: &[&str]) -> String {
    format!("Done for now.\n\n```pij\n{}\n```\n", lines.join("\n"))
}

#[tokio::test]
async fn a_block_runs_in_order_as_the_caller_and_sends_what_pij_send_sends() {
    let (addr, server, _store) = daemon().await;
    let result = turn_end(
        addr,
        &block(&[
            &format!("send {RECIPIENT} \"first, with 'quotes'\""),
            &format!("pij send {RECIPIENT} second"),
        ]),
    )
    .await;
    assert_eq!(result["outcome"], "ran", "{result}");
    assert_eq!(result["ran"], 2);
    assert_eq!(result["summary"], "pij ✓ 2 sent");
    assert_eq!(result["followup"], Value::Null);
    assert_eq!(statuses(&result), ["ran", "ran"]);
    assert!(
        result["lines"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("queued ")),
        "{result}"
    );

    // The same send through `pij send`'s own route, for comparison.
    let direct = post(
        addr,
        "/v1/shim/send",
        json!({ "argv": ["send", RECIPIENT, "third, by pij send"], "caller": { "TMUX_PANE": PANE } }),
    )
    .await;
    assert_eq!(direct.status(), reqwest::StatusCode::OK);

    let messages = inbox(addr).await;
    let bodies: Vec<&str> = messages
        .iter()
        .map(|message| message["body"].as_str().expect("body"))
        .collect();
    assert_eq!(
        bodies,
        ["first, with 'quotes'", "second", "third, by pij send"]
    );
    let shape = |message: &Value| {
        let mut message = message.clone();
        message["msg_id"] = Value::Null;
        message["body"] = Value::Null;
        message
    };
    assert_eq!(
        shape(&messages[0]),
        shape(&messages[2]),
        "same message as pij send"
    );
    assert_eq!(messages[0]["from"], SENDER, "sent as the derived caller");

    server.abort();
}

#[tokio::test]
async fn a_refusal_stops_the_block_and_names_what_already_ran() {
    let (addr, server, _store) = daemon().await;
    let result = turn_end(
        addr,
        &block(&[
            &format!("send {RECIPIENT} one"),
            "send pij-nobody-here two",
            &format!("send {RECIPIENT} three"),
        ]),
    )
    .await;
    assert_eq!(result["outcome"], "refused", "{result}");
    assert_eq!(statuses(&result), ["ran", "refused", "not_run"]);
    assert_eq!(result["ran"], 1);
    let followup = result["followup"].as_str().expect("a follow-up");
    assert!(
        followup.contains(&format!(
            "Ran, do not resend:\n  1. \"send {RECIPIENT} one\""
        )),
        "{followup}"
    );
    assert!(
        followup.contains(&format!("Not run:\n  3. \"send {RECIPIENT} three\"")),
        "{followup}"
    );
    let bodies: Vec<Value> = inbox(addr)
        .await
        .into_iter()
        .map(|m| m["body"].clone())
        .collect();
    assert_eq!(bodies, [json!("one")], "line 3 never ran");

    server.abort();
}

#[tokio::test]
async fn any_invalid_line_runs_nothing() {
    let (addr, server, _store) = daemon().await;
    for (bad, why) in [
        ("rm -rf /tmp/x", "`rm` cannot run in a block"),
        ("send pij-x 'unbalanced", "unclosed ' quote"),
        ("send pij-x --wait hi", "rs cannot serve `pij send --wait`"),
        (
            "send pij-x --body-file /etc/hosts",
            "a block cannot read a body file",
        ),
        ("requests close one two", "usage: pij requests close"),
        ("requests withdraw abc --why x", "only `requests close`"),
        (
            "output verdict --body x --body-file -",
            "give the body as --body or --body-file",
        ),
        ("send pij-x --help", "--help prints help"),
    ] {
        let result = turn_end(addr, &block(&[&format!("send {RECIPIENT} fine"), bad])).await;
        assert_eq!(result["outcome"], "invalid", "{bad}: {result}");
        assert_eq!(statuses(&result), ["not_run", "invalid"], "{bad}");
        assert_eq!(result["ran"], 0);
        let detail = result["lines"][1]["detail"].as_str().expect("detail");
        assert!(detail.contains(why), "{bad}: {detail}");
        assert!(
            result["followup"]
                .as_str()
                .is_some_and(|text| text.contains("nothing was sent")),
            "{result}"
        );
    }
    let too_many: Vec<String> = (0..21).map(|n| format!("send {RECIPIENT} n{n}")).collect();
    let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let result = turn_end(addr, &block(&refs)).await;
    assert_eq!(result["outcome"], "invalid", "{result}");
    assert_eq!(result["lines"][20]["status"], "invalid");
    assert_eq!(inbox(addr).await, Vec::<Value>::new(), "nothing ran, ever");

    server.abort();
}

/// How many FYIs are held for the recipient, claimed (and so taken) as its
/// hook would claim them.
async fn held_fyis(addr: SocketAddr) -> usize {
    let reply: Value = post(
        addr,
        "/v1/fyi/claim",
        json!({ "seat": RECIPIENT, "pane": RECIPIENT_PANE, "via": "hook:omp" }),
    )
    .await
    .json()
    .await
    .expect("claim reply");
    assert_eq!(reply["ok"], true, "{reply}");
    reply["data"]["ids"].as_array().expect("ids").len()
}

/// Review of #212 (F002): a line the send path refuses on its arguments alone
/// is invalid before anything runs, so the valid FYI ahead of it is never held.
#[tokio::test]
async fn a_send_the_send_path_refuses_on_its_arguments_alone_runs_nothing() {
    let (addr, server, _store) = daemon().await;
    for (bad, why) in [
        (
            format!("send {RECIPIENT} --force two"),
            "--force needs a non-empty --reason",
        ),
        (
            format!("send {RECIPIENT} --force --reason ' ' two"),
            "--force needs a non-empty --reason",
        ),
        (
            format!("send {RECIPIENT} --reply wake two"),
            "--reply and --limit apply to a send to role:<id>",
        ),
        (
            format!("send {RECIPIENT} --limit 5m two"),
            "--reply and --limit apply to a send to role:<id>",
        ),
        (
            format!("send {RECIPIENT} --project fleet two"),
            "--project names the directory",
        ),
        ("send 'role:no good' two".to_string(), "is not role:<id>"),
        (
            "send role:coder@elsewhere two".to_string(),
            "never forwarded",
        ),
        (
            "send role:coder --reply loud two".to_string(),
            "--reply takes wake, fyi or none",
        ),
        (
            "send role:coder --limit soon two".to_string(),
            "--limit must be a duration",
        ),
    ] {
        let text = block(&[&format!("send {RECIPIENT} --fyi 'held first'"), &bad]);
        let result = turn_end(addr, &text).await;
        assert_eq!(result["outcome"], "invalid", "{bad}: {result}");
        assert_eq!(statuses(&result), ["not_run", "invalid"], "{bad}");
        let detail = result["lines"][1]["detail"].as_str().expect("detail");
        assert!(detail.contains(why), "{bad}: {detail}");
    }
    assert_eq!(held_fyis(addr).await, 0, "no FYI was ever held");
    assert_eq!(inbox(addr).await, Vec::<Value>::new(), "nothing was sent");

    // The instrument sees a held FYI when there is one.
    let result = turn_end(addr, &block(&[&format!("send {RECIPIENT} --fyi 'held'")])).await;
    assert_eq!(result["outcome"], "ran", "{result}");
    assert_eq!(held_fyis(addr).await, 1);

    server.abort();
}

#[tokio::test]
async fn text_without_a_trailing_block_runs_nothing() {
    let (addr, server, _store) = daemon().await;
    for text in [
        "no block at all".to_string(),
        format!("Like this:\n```pij\nsend {RECIPIENT} quoted\n```\nwhich runs at turn end."),
        format!("```pij\nsend {RECIPIENT} unterminated\n"),
        // Review of #212 (F001): one ```text block whose content is a pij example.
        format!("```text\n```pij\nsend {RECIPIENT} quoted\n```"),
    ] {
        let result = turn_end(addr, &text).await;
        assert_eq!(
            result,
            json!({"outcome": "none", "lines": [], "ran": 0, "summary": null, "followup": null})
        );
    }
    assert_eq!(inbox(addr).await, Vec::<Value>::new());

    server.abort();
}

#[tokio::test]
async fn output_and_requests_close_lines_meet_their_own_routes_refusals() {
    let (addr, server, _store) = daemon().await;
    // Valid arguments, refused at run time by /v1/requests' own handler.
    let result = turn_end(
        addr,
        &block(&[
            "requests close 0199-no-such-request --why done",
            &format!("send {RECIPIENT} after"),
        ]),
    )
    .await;
    assert_eq!(statuses(&result), ["refused", "not_run"], "{result}");
    assert_eq!(result["lines"][0]["verb"], "requests close");
    assert!(result["lines"][0]["code"].is_string(), "{result}");
    // Valid arguments, refused by /v1/output's: /abs/tree is no project.
    let result = turn_end(
        addr,
        &block(&["output note --role reviewer --subject pr:1 --body hello"]),
    )
    .await;
    assert_eq!(statuses(&result), ["refused"], "{result}");
    assert_eq!(result["lines"][0]["verb"], "output");
    assert_eq!(inbox(addr).await, Vec::<Value>::new());

    server.abort();
}

#[tokio::test]
async fn a_body_outside_the_contract_is_refused() {
    let (addr, server, _store) = daemon().await;
    let response = post(
        addr,
        "/v1/turn-end",
        json!({ "text": "x", "caller": {}, "stop_hook_active": true }),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let envelope: Envelope<Value> = response.json().await.expect("envelope");
    assert!(!envelope.ok);
    assert!(
        envelope
            .meta
            .as_deref()
            .is_some_and(|meta| meta.contains("{text, caller, via}")),
        "{envelope:?}"
    );

    server.abort();
}
