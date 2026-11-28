use super::*;

const NOW: u64 = 1_800_000_000_000;

fn folder() -> (Transfers, PathBuf) {
    let dir = pij_testkit::fresh_dir("pij-transfers");
    let transfers = Transfers::new(dir.join(DIR), 0);
    transfers.prepare().expect("prepare");
    (transfers, dir)
}

fn pending(code: &str, size: u64, sha256: String, created_at: u64) -> Pending {
    Pending {
        code: code.to_string(),
        from: "pij-a".to_string(),
        from_machine: Some("laptop".to_string()),
        to: "pij-b".to_string(),
        name: "notes.txt".to_string(),
        size,
        sha256,
        note: None,
        created_at,
    }
}

async fn stored(transfers: &Transfers, dir: &Path, bytes: &[u8], created_at: u64) -> Pending {
    let source = dir.join("source.txt");
    std::fs::write(&source, bytes).expect("source");
    let code = Transfers::mint_code().expect("code");
    let (size, sha) = transfers.ingest_path(&code, &source).await.expect("ingest");
    let pending = pending(&code, size, sha, created_at);
    transfers.record(&pending).expect("record");
    pending
}

#[tokio::test]
async fn a_stored_file_is_private_and_listed_with_its_hash() {
    let (transfers, dir) = folder();
    let pending = stored(&transfers, &dir, b"hello", NOW).await;
    assert_eq!(pending.size, 5);
    assert_eq!(
        pending.sha256,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    let mode = |path: &Path| std::fs::metadata(path).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(transfers.root()), 0o700);
    assert_eq!(mode(&transfers.root().join(&pending.code)), 0o600);
    assert_eq!(
        mode(&transfers.root().join(format!("{}.json", pending.code))),
        0o600
    );
    assert_eq!(transfers.list(), vec![pending.clone()]);
    assert_eq!(transfers.get(&pending.code), Some(pending));
    assert_eq!(transfers.get("../../etc/passwd"), None);
}

#[tokio::test]
async fn more_than_the_limit_is_refused_and_leaves_nothing() {
    let (transfers, _dir) = folder();
    let code = Transfers::mint_code().expect("code");
    let mut incoming = transfers.incoming(&code, 4).await.expect("incoming");
    incoming.write(b"1234").await.expect("within");
    let refused = incoming.write(b"5").await.expect_err("over");
    assert_eq!(refused.code, E_TOO_LARGE);
    drop(incoming);
    assert_eq!(std::fs::read_dir(transfers.root()).expect("dir").count(), 0);
}

#[tokio::test]
async fn the_disk_floor_refuses_a_store() {
    let dir = pij_testkit::fresh_dir("pij-transfers-floor");
    let transfers = Transfers::new(dir.join(DIR), u64::MAX);
    transfers.prepare().expect("prepare");
    let source = dir.join("source.txt");
    std::fs::write(&source, b"x").expect("source");
    let code = Transfers::mint_code().expect("code");
    let refused = transfers
        .ingest_path(&code, &source)
        .await
        .expect_err("floor");
    assert_eq!(refused.code, E_DISK_FLOOR);
}

#[tokio::test]
async fn save_refuses_an_existing_target_and_never_follows_a_symlink() {
    let (transfers, dir) = folder();
    let pending = stored(&transfers, &dir, b"payload", NOW).await;
    let elsewhere = dir.join("elsewhere.txt");
    std::fs::write(&elsewhere, b"keep me").expect("elsewhere");
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&elsewhere, &link).expect("symlink");

    let refused = transfers
        .save(&pending, &link, false)
        .await
        .expect_err("exists");
    assert_eq!(refused.code, E_TARGET);
    let refused = transfers
        .save(&pending, &elsewhere, false)
        .await
        .expect_err("exists");
    assert_eq!(refused.code, E_TARGET);

    transfers.save(&pending, &link, true).await.expect("force");
    assert_eq!(std::fs::read(&elsewhere).expect("kept"), b"keep me");
    assert!(!std::fs::symlink_metadata(&link).expect("link").is_symlink());
    assert_eq!(std::fs::read(&link).expect("saved"), b"payload");

    let fresh = dir.join("fresh.txt");
    transfers.save(&pending, &fresh, false).await.expect("save");
    assert_eq!(std::fs::read(&fresh).expect("saved"), b"payload");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty());
}

#[tokio::test]
async fn save_refuses_bytes_that_do_not_match_their_hash() {
    let (transfers, dir) = folder();
    let pending = stored(&transfers, &dir, b"payload", NOW).await;
    std::fs::write(transfers.root().join(&pending.code), b"tampered").expect("tamper");
    let target = dir.join("out.txt");
    let refused = transfers
        .save(&pending, &target, false)
        .await
        .expect_err("corrupt");
    assert_eq!(refused.code, E_CORRUPT);
    assert!(!target.exists());
}

#[tokio::test]
async fn only_files_older_than_a_day_expire_and_a_held_one_is_spared() {
    let (transfers, dir) = folder();
    let old = stored(&transfers, &dir, b"old", NOW - files::EXPIRY_MS).await;
    let held = stored(&transfers, &dir, b"held", NOW - files::EXPIRY_MS - 1).await;
    let young = stored(&transfers, &dir, b"young", NOW - files::EXPIRY_MS + 1).await;
    let hold = transfers.hold(&held.code).expect("hold");
    assert!(transfers.hold(&held.code).is_none());

    assert_eq!(transfers.take_expired(NOW), vec![old.clone()]);
    assert!(!transfers.root().join(&old.code).exists());
    assert_eq!(transfers.list(), vec![held.clone(), young]);
    drop(hold);
    assert_eq!(transfers.take_expired(NOW), vec![held]);
}
