use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;

use super::{HealthProbe, probe_unauthenticated, reproducible_repo};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct RepoFixture {
    root: PathBuf,
}

impl RepoFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pij-bounce-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("fixture dir");
        let root = root.canonicalize().expect("canonical fixture dir");
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.email", "pij@example.invalid"]);
        git(&root, &["config", "user.name", "pij test"]);
        std::fs::write(root.join("tracked"), "one\n").expect("tracked");
        git(&root, &["add", "tracked"]);
        git(&root, &["commit", "-m", "initial"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        Self { root }
    }
}

impl Drop for RepoFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clean_head_equal_to_origin_main_is_reproducible() {
    let fixture = RepoFixture::new();
    assert_eq!(
        reproducible_repo(&fixture.root).expect("clean main"),
        fixture.root
    );
}

#[test]
fn dirty_and_divergent_are_named_as_different_refusals() {
    let fixture = RepoFixture::new();
    std::fs::write(fixture.root.join("untracked"), "dirty\n").expect("untracked");
    let dirty = reproducible_repo(&fixture.root).expect_err("dirty must refuse");
    assert!(dirty.to_string().contains("checkout is dirty"), "{dirty}");

    std::fs::remove_file(fixture.root.join("untracked")).expect("remove dirty file");
    std::fs::write(fixture.root.join("tracked"), "two\n").expect("change tracked");
    git(&fixture.root, &["add", "tracked"]);
    git(&fixture.root, &["commit", "-m", "unpushed"]);
    let divergent = reproducible_repo(&fixture.root).expect_err("divergent must refuse");
    assert!(divergent.to_string().contains("HEAD"), "{divergent}");
    assert!(divergent.to_string().contains("origin/main"), "{divergent}");
}

async fn server(status: StatusCode) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let joined = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/health", get(move || async move { status })),
        )
        .await
        .expect("serve");
    });
    (addr, joined)
}

#[tokio::test]
async fn unauthenticated_401_is_distinct_from_no_listener_and_wrong_status() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .expect("client");

    let (enforcing_addr, enforcing) = server(StatusCode::UNAUTHORIZED).await;
    assert!(matches!(
        probe_unauthenticated(&client, enforcing_addr).await,
        HealthProbe::Enforcing
    ));
    enforcing.abort();

    let (wrong_addr, wrong) = server(StatusCode::OK).await;
    assert!(matches!(
        probe_unauthenticated(&client, wrong_addr).await,
        HealthProbe::Unexpected(StatusCode::OK)
    ));
    wrong.abort();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve unused port");
    let absent_addr = listener.local_addr().expect("address");
    drop(listener);
    assert!(matches!(
        probe_unauthenticated(&client, absent_addr).await,
        HealthProbe::Absent(_)
    ));
}
