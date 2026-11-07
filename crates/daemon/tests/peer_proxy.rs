//! Review S4 (plan 164): the federation client connects to a peer DIRECTLY.
//! A proxy named in the environment would otherwise receive every request,
//! bearer key included. The proxy variables are set on a CHILD run of this
//! test binary (`unsafe_code` is forbidden, so this process never sets them).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pij_core::config::PeerDefinition;
use pij_core::model::Destination;
use pij_daemon::events::EventBus;
use pij_daemon::federation::{FederationPolicy, FederationService};
use pij_daemon::http::SendRequest;
use pij_testkit::fakes::{FakeQueue, FakeSpine};
use tokio::io::AsyncReadExt;

const INNER: &str = "PIJ_PROXY_TEST_INNER";

#[tokio::test]
async fn the_federation_client_ignores_proxy_environment() {
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy");
    let proxy_addr = proxy.local_addr().expect("addr");
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorded = Arc::clone(&seen);
    let recorder = tokio::spawn(async move {
        while let Ok((mut stream, _)) = proxy.accept().await {
            let mut buffer = vec![0_u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            recorded
                .lock()
                .expect("record")
                .push(String::from_utf8_lossy(&buffer[..read]).into_owned());
        }
    });
    let mut child = tokio::process::Command::new(std::env::current_exe().expect("test binary"));
    child
        .args([
            "--exact",
            "forward_once_under_the_inherited_environment",
            "--nocapture",
        ])
        .env(INNER, "1");
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
    ] {
        child.env(name, format!("http://{proxy_addr}"));
    }
    let output = child.output().await.expect("child run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    recorder.abort();
    let seen = seen.lock().expect("seen");
    assert!(
        seen.is_empty(),
        "the proxy received {} federation request(s): {:?}",
        seen.len(),
        seen.iter()
            .map(|request| request.lines().next().unwrap_or_default())
            .collect::<Vec<_>>()
    );
}

/// The child half: a no-op unless launched by the test above.
#[tokio::test]
async fn forward_once_under_the_inherited_environment() {
    if std::env::var_os(INNER).is_none() {
        return;
    }
    let queue = Arc::new(FakeQueue::new(64).expect("queue"));
    let bus = Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 16).expect("bus"));
    let federation = FederationService::new(
        "studio".to_string(),
        [PeerDefinition {
            alias: "laptop".to_string(),
            // Unresolvable directly; only a proxy could "reach" it.
            url: "http://laptop.invalid:9".to_string(),
            key: "generated-test-key-0123456789abcdef0123".to_string(),
        }],
        queue,
        bus,
        FederationPolicy {
            poll_interval: Duration::from_millis(10),
            max_retry_delay: Duration::from_millis(100),
            event_buffer_capacity: 16,
            first_attempt_wait: Duration::from_millis(10),
        },
    )
    .expect("federation");
    federation
        .enqueue_remote(&SendRequest {
            from: "pij-a".into(),
            to: Destination {
                seat: "pij-b".into(),
                machine: Some("laptop".to_string()),
            },
            body: "hello".to_string(),
            msg_id: "m-proxy".to_string(),
            from_machine: None,
            in_reply_to: None,
            fyi: false,
            force: false,
            reason: None,
        })
        .await
        .expect("queued");
    let _ = federation.process_one().await;
}
