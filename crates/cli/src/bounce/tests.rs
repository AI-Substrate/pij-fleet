use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;

use super::{HealthProbe, install, probe_unauthenticated, reproducible_repo};

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

/// A fake release build in the fixture's target/ that prints `word`.
fn build_fake(root: &Path, word: &str) {
    let release = root.join("target/release");
    std::fs::create_dir_all(&release).expect("target/release");
    let artifact = release.join("pij-rs");
    std::fs::write(&artifact, format!("#!/bin/sh\necho {word}\n")).expect("fake build");
    let mut permissions = std::fs::metadata(&artifact)
        .expect("fake build")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&artifact, permissions).expect("make the build executable");
}

/// The fixture with the repository's own installer in it, and a home.
fn with_installer(fixture: &RepoFixture) -> PathBuf {
    let scripts = fixture.root.join("harness/scripts");
    std::fs::create_dir_all(&scripts).expect("scripts dir");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/scripts/install-pij-rs.sh"),
        scripts.join("install-pij-rs.sh"),
    )
    .expect("the installer");
    let home = fixture.root.join("home");
    std::fs::create_dir_all(&home).expect("home");
    home
}

fn mode(path: &Path) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(path).expect("metadata").permissions(),
    ) & 0o777
}

fn inode(path: &Path) -> u64 {
    std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(path).expect("metadata"))
}

/// F187: the daemon is launched from a copy outside target/, installed by the
/// repository's own script, so deleting target/ can't break its next launch.
/// An unchanged build leaves the copy alone, and a changed one replaces it by
/// rename (#229 review, P229-F001); the directories it makes are private
/// (S229-001).
#[test]
fn install_copies_the_build_outside_target_and_replaces_it_by_rename() {
    let fixture = RepoFixture::new();
    let home = with_installer(&fixture);

    build_fake(&fixture.root, "one");
    let copy = install(&fixture.root, &home).expect("installed");
    assert_eq!(copy, home.join(".local/share/pij/bin/pij-rs"));
    for dir in [
        ".local",
        ".local/share",
        ".local/share/pij",
        ".local/share/pij/bin",
    ] {
        assert_eq!(mode(&home.join(dir)), 0o700, "{dir} is private");
    }
    // A daemon running the first copy holds its file.
    let running = fixture.root.join("running");
    std::fs::hard_link(&copy, &running).expect("hold the running file");

    build_fake(&fixture.root, "one");
    install(&fixture.root, &home).expect("reinstalled unchanged");
    assert_eq!(
        inode(&copy),
        inode(&running),
        "an unchanged build leaves the running file in place"
    );

    build_fake(&fixture.root, "two");
    assert_eq!(install(&fixture.root, &home).expect("reinstalled"), copy);
    assert_ne!(inode(&copy), inode(&running), "a new build is a new file");
    assert_eq!(
        std::fs::read_to_string(&running).expect("the running file"),
        "#!/bin/sh\necho one\n",
        "the running file is never rewritten in place"
    );
    let siblings = std::fs::read_dir(copy.parent().expect("bin dir"))
        .expect("bin dir")
        .count();
    assert_eq!(siblings, 1, "no temporary file is left beside the copy");

    std::fs::remove_dir_all(fixture.root.join("target")).expect("delete target/");
    let output = Command::new(&copy).output().expect("run the copy");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "two\n");

    let error = install(&fixture.root, &home).expect_err("no build to install");
    assert!(error.to_string().contains("no built pij-rs"), "{error}");
    assert!(copy.is_file(), "a failed install leaves the copy alone");
}

/// #229 review, S229-001: the installer refuses an install directory, existing
/// or on the way, that someone else can write to, and installs nothing.
#[test]
fn install_refuses_a_directory_others_can_write_to() {
    for (dir, writable) in [(".local/share", 0o775), (".local/share/pij/bin", 0o777)] {
        let fixture = RepoFixture::new();
        let home = with_installer(&fixture);
        build_fake(&fixture.root, "one");
        let open = home.join(dir);
        std::fs::create_dir_all(&open).expect("the open directory");
        let mut permissions = std::fs::metadata(&open).expect("dir").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, writable);
        std::fs::set_permissions(&open, permissions).expect("open it up");

        let error = install(&fixture.root, &home).expect_err("refused");
        assert!(
            error.to_string().contains("refusing") && error.to_string().contains(dir),
            "{dir}: {error}"
        );
        assert!(
            !home.join(".local/share/pij/bin/pij-rs").exists(),
            "{dir}: nothing installed"
        );
    }
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
