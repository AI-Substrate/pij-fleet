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

    let marker = git_dir(&scratch.gov).unwrap().join(MARKER);
    assert_eq!(
        marker.parent(),
        lock.parent(),
        "the marker sits beside the index.lock it explains"
    );
    assert!(!marker.exists(), "a refused write leaves no marker");

    // Released while the writer waits: the write lands. The marker names the
    // writer while it waits, and is gone after.
    let release = {
        let (lock, marker) = (lock.clone(), marker.clone());
        std::thread::spawn(move || {
            // The writer marks the index before its first `git add` attempt.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !marker.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let named = std::fs::read_to_string(&marker).unwrap_or_default();
            std::fs::remove_file(lock).unwrap();
            named
        })
    };
    writer(vec![], &LOCK_RETRY)
        .write(&scratch.main, role("qa", "{}"), "add")
        .expect("lands once the lock is released");
    let named = release.join().unwrap();
    assert!(
        named.starts_with("pij-rs daemon pid=") && named.contains("writing \"add\""),
        "{named:?}"
    );
    assert!(
        !marker.exists(),
        "the marker is removed when the write ends"
    );
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

// Review of #114 (pij-still-termite): the reviewer's regressions, kept.

fn review_real_role(id: &str, brief: Option<&str>) -> Vec<NewFile> {
    let fields = crate::community::role::RoleFields {
        title: Some(id.into()),
        kind: Some("general".into()),
        lifecycle: Some("per-task".into()),
        holders: Some("many".into()),
        rules: vec!["Review scratch proof.".into()],
        packet: brief.is_none().then(|| "A real inline packet.".into()),
        brief: brief.map(str::to_string),
        ..Default::default()
    };
    let document = crate::community::role::stamp(
        crate::community::role::from_fields(id, &fields).unwrap(),
        id,
        "pij-still-termite",
        "review probe",
        "2026-11-04",
    )
    .unwrap();
    role(
        id,
        &String::from_utf8(crate::community::role::to_bytes(&document)).unwrap(),
    )
}

fn index(scratch: &Scratch) -> String {
    scratch.git(&scratch.gov, &["diff", "--cached", "--name-only"])
}

#[test]
fn review_real_ddocs_existing_brief_control() {
    let ddocs = crate::community::ddocs::Ddocs::from_env();
    if ddocs.path().is_none() {
        eprintln!("SKIP review_real_ddocs_existing_brief_control: ddocs is not installed here");
        return;
    }
    let scratch = Scratch::new("pr114-review-existing-brief");
    std::fs::create_dir(scratch.gov.join("briefs")).unwrap();
    std::fs::write(scratch.gov.join("briefs/qa.md"), "A standing packet.\n").unwrap();
    scratch.git(&scratch.gov, &["add", "briefs/qa.md"]);
    scratch.git(&scratch.gov, &["commit", "-q", "-m", "brief"]);
    let writer = CommunityWriter::new(Arc::new(ddocs), layout::schema_copies(), &LOCK_RETRY);
    writer
        .write(
            &scratch.main,
            review_real_role("qa", Some("briefs/qa.md")),
            "existing brief",
        )
        .expect("an existing repo-relative brief is valid");
}

#[test]
fn review_real_ddocs_missing_brief_refuses_entire_batch() {
    let scratch = Scratch::new("pr114-review-missing-brief");
    let start = head(&scratch);
    let writer = CommunityWriter::new(
        Arc::new(crate::community::ddocs::Ddocs::from_env()),
        layout::schema_copies(),
        &LOCK_RETRY,
    );
    let mut files = review_real_role("inline", None);
    files.extend(review_real_role(
        "missing",
        Some("briefs/definitely-absent.md"),
    ));
    let result = writer.write(&scratch.main, files, "one missing packet");
    assert!(
        result.is_err(),
        "a standing packet must reference an existing brief, and a bad batch must commit none; got {result:?}"
    );
    assert_eq!(head(&scratch), start);
    assert_eq!(status(&scratch), "");
    assert_eq!(index(&scratch), "");
}

#[test]
fn review_commit_failure_preserves_foreign_index_and_worktree() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("pr114-review-commit-failure");
    std::fs::write(scratch.gov.join("foreign.md"), "committed\n").unwrap();
    scratch.git(&scratch.gov, &["add", "foreign.md"]);
    scratch.git(&scratch.gov, &["commit", "-q", "-m", "foreign base"]);
    std::fs::write(scratch.gov.join("foreign.md"), "staged edit\n").unwrap();
    scratch.git(&scratch.gov, &["add", "foreign.md"]);
    std::fs::write(scratch.gov.join("foreign.md"), "unstaged edit\n").unwrap();
    let hooks = scratch.root.join("hooks");
    std::fs::create_dir(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    scratch.git(
        &scratch.gov,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let before_head = head(&scratch);
    let before_status = status(&scratch);
    let before_index = scratch.git(&scratch.gov, &["show", ":foreign.md"]);
    let result = writer(vec![], &LOCK_RETRY).write(&scratch.main, role("qa", "{}"), "fails");
    assert_eq!(result.unwrap_err().code, E_GOV_GIT);
    assert_eq!(head(&scratch), before_head);
    assert_eq!(status(&scratch), before_status);
    assert_eq!(
        scratch.git(&scratch.gov, &["show", ":foreign.md"]),
        before_index
    );
    assert_eq!(
        std::fs::read_to_string(scratch.gov.join("foreign.md")).unwrap(),
        "unstaged edit\n"
    );
}

/// F001 without ddocs (so it runs in CI): a brief that is missing, outside the
/// worktree, absolute, or a symlink out of it refuses the whole batch by name,
/// before anything is written; an existing brief lands.
#[test]
fn a_brief_that_is_not_a_file_inside_the_worktree_refuses_the_whole_batch_before_writing() {
    let scratch = Scratch::new("pij-writer-brief");
    let outside = scratch.root.join("outside.md");
    std::fs::write(&outside, "outside\n").unwrap();
    std::fs::create_dir(scratch.gov.join("briefs")).unwrap();
    std::fs::write(scratch.gov.join("briefs/qa.md"), "A standing packet.\n").unwrap();
    std::os::unix::fs::symlink(&outside, scratch.gov.join("briefs/link.md")).unwrap();
    scratch.git(&scratch.gov, &["add", "briefs"]);
    scratch.git(&scratch.gov, &["commit", "-q", "-m", "briefs"]);
    let start = head(&scratch);
    let writer = writer(vec![], &LOCK_RETRY);
    for brief in [
        "briefs/absent.md",
        "../outside.md",
        outside.to_str().unwrap(),
        "briefs/link.md",
        "briefs",
    ] {
        let mut files = review_real_role("inline", None);
        files.extend(review_real_role("missing", Some(brief)));
        let refusal = writer
            .write(&scratch.main, files, "batch")
            .expect_err(brief);
        assert_eq!(refusal.code, crate::community::E_BRIEF_MISSING, "{brief}");
        assert!(
            refusal.message.contains("\"missing\"") && refusal.message.contains(brief),
            "{refusal}"
        );
        assert_eq!(head(&scratch), start, "{brief}");
        assert_eq!(status(&scratch), "", "{brief}");
        assert_eq!(index(&scratch), "", "{brief}");
    }
    writer
        .write(
            &scratch.main,
            review_real_role("qa", Some("briefs/qa.md")),
            "existing brief",
        )
        .expect("an existing brief inside the worktree lands");
}

// Review of #114, S1 (pij-wary-lynx): a clean, tracked symlink is no foreign
// edit to git, so the writer itself refuses any destination that is, or sits
// under, a symlink. Nothing may land outside the worktree, and the link stays.

fn commit_link(scratch: &Scratch, at: &str, to: &Path) {
    let link = scratch.gov.join(at);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(to, &link).unwrap();
    scratch.git(&scratch.gov, &["add", at]);
    scratch.git(&scratch.gov, &["commit", "-q", "-m", "link"]);
}

fn assert_refused_untouched(
    scratch: &Scratch,
    refusal: Refusal,
    link: &str,
    outside: &Path,
    start: &str,
) {
    assert_eq!(refusal.code, crate::community::E_GOV_SYMLINK, "{refusal}");
    assert!(refusal.message.contains(link), "{refusal}");
    assert_eq!(head(scratch), start);
    assert_eq!(status(scratch), "");
    assert!(
        std::fs::symlink_metadata(scratch.gov.join(link))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the tracked link survives"
    );
    let escaped: Vec<_> = std::fs::read_dir(outside)
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default();
    assert!(escaped.is_empty(), "written outside the root: {escaped:?}");
}

#[test]
fn a_dangling_tracked_symlink_at_the_destination_refuses_and_nothing_escapes() {
    let scratch = Scratch::new("pij-writer-link-file");
    let outside = scratch.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = ".harness/government/community/roles/qa/role.dd.json";
    commit_link(&scratch, link, &outside.join("role.json"));
    let start = head(&scratch);
    let refusal = writer(vec![], &LOCK_RETRY)
        .write(&scratch.main, role("qa", "{}"), "add")
        .unwrap_err();
    assert_refused_untouched(&scratch, refusal, link, &outside, &start);
}

#[test]
fn a_symlinked_ancestor_directory_refuses_and_nothing_escapes() {
    let scratch = Scratch::new("pij-writer-link-dir");
    let outside = scratch.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = ".harness/government/community/roles/qa";
    commit_link(&scratch, link, &outside);
    let start = head(&scratch);
    let refusal = writer(vec![], &LOCK_RETRY)
        .write(&scratch.main, role("qa", "{}"), "add")
        .unwrap_err();
    assert_refused_untouched(&scratch, refusal, link, &outside, &start);
}

/// The rendered sibling and a schema copy are destinations too: a link at
/// either refuses the whole write before the role document is written.
#[test]
fn a_symlinked_rendered_sibling_or_schema_copy_refuses_before_anything_is_written() {
    for (name, link, schemas) in [
        (
            "pij-writer-link-md",
            ".harness/government/community/roles/qa/role.dd.md",
            vec![],
        ),
        (
            "pij-writer-link-schema",
            ".dd/schemas/pij/role/schema.json",
            vec![(PathBuf::from(".dd/schemas/pij/role/schema.json"), SCHEMA)],
        ),
        (
            "pij-writer-link-schema-dir",
            ".dd",
            vec![(PathBuf::from(".dd/schemas/pij/role/schema.json"), SCHEMA)],
        ),
    ] {
        let scratch = Scratch::new(name);
        let outside = scratch.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let target = if link == ".dd" {
            outside.clone()
        } else {
            outside.join("target")
        };
        commit_link(&scratch, link, &target);
        let start = head(&scratch);
        let refusal = writer(schemas, &LOCK_RETRY)
            .write(&scratch.main, role("qa", "{}"), "add")
            .unwrap_err();
        assert!(
            !scratch
                .gov
                .join(".harness/government/community/roles/qa/role.dd.json")
                .exists(),
            "{name}: the role document was written"
        );
        assert_refused_untouched(&scratch, refusal, link, &outside, &start);
    }
}
