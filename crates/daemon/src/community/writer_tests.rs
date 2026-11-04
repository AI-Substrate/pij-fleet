//! The writer against real scratch git repositories (plan 190, AC2 and AC4).
//! git is never faked; ddocs is, through the constructor.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::community::E_DDOCS_INVALID;
use crate::community::layout::RolePaths;
use crate::community::test_repo::Scratch;

/// Refuses a document holding `INVALID`; renders every other one.
struct FakeDdocs;

impl Validator for FakeDdocs {
    fn validate(&self, _worktree: &Path, document: &Path) -> Result<(), Refusal> {
        let text = std::fs::read_to_string(document).unwrap();
        if text.contains("INVALID") {
            return Err(Refusal::new(
                E_DDOCS_INVALID,
                "missing required field \"scope\"",
            ));
        }
        Ok(())
    }

    fn build(&self, _worktree: &Path, document: &Path) -> Result<(), Refusal> {
        std::fs::write(rendered(document), "rendered\n").unwrap();
        Ok(())
    }
}

const SCHEMA: &[u8] = b"{\"schema\":\"pij/role\"}\n";

fn writer(schemas: Vec<(PathBuf, &'static [u8])>, retry: &[Duration]) -> CommunityWriter {
    CommunityWriter::new(Arc::new(FakeDdocs), schemas, retry)
}

fn role(id: &str, body: &str) -> Vec<NewFile> {
    let paths = RolePaths::new(id).unwrap();
    vec![
        NewFile {
            path: paths.document.clone(),
            bytes: body.as_bytes().to_vec(),
            document: true,
            existing: Existing::Refuse(layout::exists(id, &paths.document)),
        },
        NewFile {
            path: paths.memory_index,
            bytes: RolePaths::empty_memory_index(id).into_bytes(),
            document: false,
            existing: Existing::Refuse(layout::exists(id, &paths.document)),
        },
    ]
}

fn status(scratch: &Scratch) -> String {
    scratch.git(
        &scratch.gov,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
}

fn head(scratch: &Scratch) -> String {
    scratch
        .git(&scratch.gov, &["rev-parse", "HEAD"])
        .trim()
        .to_string()
}

/// AC4: unstaged, staged and untracked edits all survive a write exactly, and
/// the checkout's next ordinary commit keeps what pij wrote.
#[test]
fn a_write_commits_only_its_own_paths_and_leaves_every_other_edit_as_it_was() {
    let scratch = Scratch::new("pij-writer-safety");
    let gov = scratch.gov.clone();
    for name in ["unstaged.md", "staged.md"] {
        std::fs::write(gov.join(name), "committed\n").unwrap();
        scratch.git(&gov, &["add", name]);
    }
    scratch.git(&gov, &["commit", "-q", "-m", "two files"]);
    std::fs::write(gov.join("unstaged.md"), "prime's unstaged edit\n").unwrap();
    std::fs::write(gov.join("staged.md"), "prime's staged edit\n").unwrap();
    scratch.git(&gov, &["add", "staged.md"]);
    std::fs::write(gov.join("untracked.md"), "prime's new file\n").unwrap();
    let before = status(&scratch);

    let written = writer(vec![], &LOCK_RETRY)
        .write(
            &scratch.main,
            role("security", "{}"),
            "community: add role security",
        )
        .expect("the write lands");

    assert_eq!(written.commit, head(&scratch));
    let committed = scratch.git(&gov, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed.lines().collect::<Vec<_>>(),
        [
            ".harness/government/community/roles/security/memory/INDEX.md",
            ".harness/government/community/roles/security/role.dd.json",
            ".harness/government/community/roles/security/role.dd.md",
        ]
    );
    assert_eq!(
        status(&scratch),
        before,
        "staged stays staged, unstaged stays unstaged"
    );
    assert_eq!(
        std::fs::read_to_string(gov.join("unstaged.md")).unwrap(),
        "prime's unstaged edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(gov.join("untracked.md")).unwrap(),
        "prime's new file\n"
    );

    // The prime's own next commit neither deletes nor reverts pij's files.
    scratch.git(&gov, &["commit", "-q", "-m", "prime's commit"]);
    let tree = scratch.git(&gov, &["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(
        tree.contains("community/roles/security/role.dd.json"),
        "{tree}"
    );
    assert_eq!(
        scratch.git(
            &gov,
            &[
                "show",
                "HEAD:.harness/government/community/roles/security/role.dd.json"
            ]
        ),
        "{}"
    );
    assert!(
        !status(&scratch).contains("security"),
        "{}",
        status(&scratch)
    );
}

#[test]
fn a_path_carrying_a_foreign_edit_refuses_and_writes_nothing() {
    let scratch = Scratch::new("pij-writer-foreign");
    let gov = scratch.gov.clone();
    let writer = writer(vec![], &LOCK_RETRY);
    writer
        .write(&scratch.main, role("pm", "{}"), "community: add role pm")
        .unwrap();
    let paths = RolePaths::new("qa").unwrap();
    let document = gov.join(&paths.document);
    std::fs::create_dir_all(document.parent().unwrap()).unwrap();
    let start = head(&scratch);

    // Untracked.
    std::fs::write(&document, "someone's draft\n").unwrap();
    let refusal = writer
        .write(&scratch.main, role("qa", "{}"), "x")
        .unwrap_err();
    assert_eq!(refusal.code, E_FOREIGN_EDIT);
    assert!(refusal.message.contains("role.dd.json (??)"), "{refusal}");
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        "someone's draft\n"
    );

    // Staged.
    scratch.git(&gov, &["add", paths.document.to_str().unwrap()]);
    assert_eq!(
        writer
            .write(&scratch.main, role("qa", "{}"), "x")
            .unwrap_err()
            .code,
        E_FOREIGN_EDIT
    );

    // Modified, on a tracked file pij wrote itself.
    scratch.git(&gov, &["reset", "-q"]);
    std::fs::remove_file(&document).unwrap();
    let pm = RolePaths::new("pm").unwrap().memory_index;
    std::fs::write(gov.join(&pm), "hand edit\n").unwrap();
    let mut files = role("pm", "{}");
    for file in &mut files {
        file.existing = Existing::Replace;
    }
    let refusal = writer.write(&scratch.main, files, "x").unwrap_err();
    assert_eq!(refusal.code, E_FOREIGN_EDIT);
    assert!(refusal.message.contains("INDEX.md (M)"), "{refusal}");
    assert_eq!(
        std::fs::read_to_string(gov.join(&pm)).unwrap(),
        "hand edit\n"
    );
    assert_eq!(head(&scratch), start);
}

#[test]
fn adding_an_existing_role_refuses_by_name_and_changes_nothing() {
    let scratch = Scratch::new("pij-writer-exists");
    let writer = writer(vec![], &LOCK_RETRY);
    writer
        .write(&scratch.main, role("pm", "{\"v\":1}"), "add")
        .unwrap();
    let start = head(&scratch);
    let refusal = writer
        .write(&scratch.main, role("pm", "{\"v\":2}"), "add again")
        .unwrap_err();
    assert_eq!(refusal.code, crate::community::E_DIRECTORY_EXISTS);
    assert_eq!(head(&scratch), start);
    assert_eq!(status(&scratch), "");
    let document = scratch.gov.join(RolePaths::new("pm").unwrap().document);
    assert_eq!(std::fs::read_to_string(document).unwrap(), "{\"v\":1}");
}

#[test]
fn a_document_ddocs_rejects_commits_nothing_and_leaves_no_trace() {
    let scratch = Scratch::new("pij-writer-invalid");
    let start = head(&scratch);
    let refusal = writer(vec![], &LOCK_RETRY)
        .write(&scratch.main, role("qa", "INVALID"), "add")
        .unwrap_err();
    assert_eq!(refusal.code, E_DDOCS_INVALID);
    assert_eq!(
        refusal.message,
        ".harness/government/community/roles/qa/role.dd.json: missing required field \"scope\""
    );
    assert_eq!(head(&scratch), start);
    assert_eq!(status(&scratch), "");
    assert!(
        !scratch
            .gov
            .join(".harness/government/community/roles")
            .exists()
    );
}

#[test]
fn two_concurrent_writes_both_land_one_after_the_other() {
    let scratch = Scratch::new("pij-writer-concurrent");
    let start = head(&scratch);
    let writer = Arc::new(writer(vec![], &LOCK_RETRY));
    let handles: Vec<_> = ["alpha", "beta"]
        .into_iter()
        .map(|id| {
            let writer = Arc::clone(&writer);
            let main = scratch.main.clone();
            std::thread::spawn(move || writer.write(&main, role(id, "{}"), &format!("add {id}")))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().expect("each write lands");
    }
    let log = scratch.git(&scratch.gov, &["rev-list", &format!("{start}..HEAD")]);
    assert_eq!(log.lines().count(), 2);
    let tree = scratch.git(&scratch.gov, &["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(tree.contains("roles/alpha/role.dd.json") && tree.contains("roles/beta/role.dd.json"));
    assert_eq!(status(&scratch), "");
}

#[test]
fn a_held_index_lock_is_waited_for_then_refused_by_name_never_forced() {
    let scratch = Scratch::new("pij-writer-lock");
    let lock = PathBuf::from(
        scratch
            .git(
                &scratch.gov,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "index.lock",
                ],
            )
            .trim(),
    );
    let start = head(&scratch);
    std::fs::write(&lock, "").unwrap();
    let short = [Duration::from_millis(20), Duration::from_millis(20)];
    let refusal = writer(vec![], &short)
        .write(&scratch.main, role("qa", "{}"), "add")
        .unwrap_err();
    assert_eq!(refusal.code, E_GOV_LOCKED);
    assert!(lock.exists(), "pij never removes another process's lock");
    assert_eq!(head(&scratch), start);
    assert!(
        !scratch
            .gov
            .join(".harness/government/community/roles/qa")
            .exists()
    );

    // Released while the writer waits: the write lands.
    let release = {
        let lock = lock.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::remove_file(lock).unwrap();
        })
    };
    writer(vec![], &LOCK_RETRY)
        .write(&scratch.main, role("qa", "{}"), "add")
        .expect("lands once the lock is released");
    release.join().unwrap();
    assert_ne!(head(&scratch), start);
}

/// Ruling C9: stale or absent schema copies are refreshed in the same commit;
/// current ones are not rewritten.
#[test]
fn schema_copies_ride_in_the_same_commit_only_when_they_differ() {
    let scratch = Scratch::new("pij-writer-schemas");
    let copy = PathBuf::from(".dd/schemas/pij/role/schema.json");
    let writer = writer(vec![(copy.clone(), SCHEMA)], &LOCK_RETRY);
    writer
        .write(&scratch.main, role("pm", "{}"), "add pm")
        .unwrap();
    let first = scratch.git(&scratch.gov, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        first.contains(".dd/schemas/pij/role/schema.json"),
        "{first}"
    );
    assert_eq!(std::fs::read(scratch.gov.join(&copy)).unwrap(), SCHEMA);

    writer
        .write(&scratch.main, role("qa", "{}"), "add qa")
        .unwrap();
    let second = scratch.git(&scratch.gov, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(!second.contains("schema.json"), "{second}");
}
