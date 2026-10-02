#[path = "../src/orchestration/repo.rs"]
mod repo;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pij_core::model::SeatId;
use pij_core::orchestration::{Project, mint_ordinal, plan_stream_creation};
use pij_store::SqliteOrchestration;
use pij_testkit::{FreshStore, fresh_dir};
use repo::{StreamCreation, reserve_and_create_stream, scan_repo_inventory};

struct FixtureRepo {
    root: PathBuf,
    cleanup_root: PathBuf,
}

impl FixtureRepo {
    fn new() -> Self {
        let cleanup_root = fresh_dir("pij-orchestration-git");
        let root = cleanup_root.join("s007-clone");
        fs::create_dir_all(&root).expect("clone dir");
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "pij-test@example.invalid"]);
        git(&root, &["config", "user.name", "pij test"]);
        fs::write(root.join("tracked.txt"), "base\n").expect("fixture file");
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-m", "fixture"]);
        Self { root, cleanup_root }
    }

    fn add_all_three_namespaces(&self) {
        let linked = self.cleanup_root.join("s011-linked");
        git(
            &self.root,
            &[
                "worktree",
                "add",
                "-b",
                "s011/linked",
                linked.to_str().expect("utf8"),
                "HEAD",
            ],
        );
        git(&self.root, &["branch", "s013/head", "HEAD"]);
    }
}

impl Drop for FixtureRepo {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["worktree", "remove", "--force"])
            .arg(self.cleanup_root.join("s011-linked"))
            .output();
        let _ = fs::remove_dir_all(&self.cleanup_root);
    }
}

#[test]
fn ordinal_scan_covers_clone_linked_worktrees_and_unchecked_out_heads() {
    let fixture = FixtureRepo::new();
    fixture.add_all_three_namespaces();

    let inventory = scan_repo_inventory(&fixture.root).expect("scan");
    assert_eq!(inventory.clone.iter().copied().collect::<Vec<_>>(), vec![7]);
    assert!(inventory.worktrees.contains(&11));
    assert!(inventory.branch_heads.contains(&13));
    assert_eq!(mint_ordinal(&inventory), Ok(14));
}

#[tokio::test]
async fn dirty_source_tree_does_not_block_stream_creation() {
    let fixture = FixtureRepo::new();
    fixture.add_all_three_namespaces();
    fs::write(
        fixture.root.join("tracked.txt"),
        "dirty and deliberately uncommitted\n",
    )
    .expect("dirty source");
    let status = git_output(&fixture.root, &["status", "--porcelain"]);
    assert!(!status.trim().is_empty(), "fixture must actually be dirty");

    let inventory = scan_repo_inventory(&fixture.root).expect("scan");
    let worktree_root = fixture.cleanup_root.join("streams");
    fs::create_dir_all(&worktree_root).expect("stream root");
    let plan = plan_stream_creation(
        "rust-port",
        "orchestration",
        &worktree_root,
        "HEAD",
        &inventory,
    )
    .expect("plan");
    assert_eq!(plan.ordinal, 14);

    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let store = SqliteOrchestration::new(pool.clone());
    let actor = SeatId::from("pij-pm");
    assert!(
        store
            .create_project(&Project {
                slug: "rust-port".to_string(),
                description: None,
                repo: None,
                plan_path: None,
                prime_id: None,
                created_by: actor.clone(),
                created_at: 1,
            })
            .await
            .expect("project")
    );
    assert_eq!(
        reserve_and_create_stream(&fixture.root, &store, &plan, &actor, 2)
            .await
            .expect("dirty source is irrelevant"),
        StreamCreation::Created
    );
    assert!(plan.worktree.join(".git").exists());
    assert_eq!(
        git_output(&plan.worktree, &["branch", "--show-current"]).trim(),
        "s014/orchestration"
    );
    let reserved: i64 = sqlx::query_scalar("SELECT count(*) FROM streams WHERE ordinal=14")
        .fetch_one(&pool)
        .await
        .expect("reservation");
    assert_eq!(reserved, 1, "reservation was durable before Git ran");
    assert!(
        !git_output(&fixture.root, &["status", "--porcelain"])
            .trim()
            .is_empty(),
        "stream creation must not clean or otherwise mutate source-tree dirt"
    );
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {} failed", args.join(" "));
    String::from_utf8(output.stdout).expect("git utf8")
}
