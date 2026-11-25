//! Rebuild the store's community pointers from the governance branch head
//! (plan 190, AC7). Two git launches, on demand only.

use std::collections::BTreeMap;
use std::path::Path;

use pij_store::CommunityPointer;

use super::git::{Locks, git};
use super::layout::{COMMUNITY, is_document};
use super::root::GovernanceRoot;
use super::{E_GOV_GIT, Refusal};

/// The pointers a write records: one per community document it committed,
/// each role document with its type read from `worktree` (PRD 006, plan 237).
pub fn for_write(
    project: &str,
    paths: &[String],
    commit: &str,
    worktree: &Path,
) -> Vec<CommunityPointer> {
    paths
        .iter()
        .filter(|path| is_document(path))
        .map(|path| CommunityPointer {
            project: project.to_string(),
            path: path.clone(),
            commit: commit.to_string(),
            role_type: role_type_at(worktree, path),
        })
        .collect()
}

/// `<prefix><role><suffix>`: how a role document's pointer path is built, for
/// the store's join from holdings to types.
pub fn role_document_affixes() -> (String, String) {
    (format!("{COMMUNITY}/roles/"), "/role.dd.json".to_string())
}

/// A role document's `meta.type` in `worktree`, or `None` for any other
/// document, an unreadable one or an untyped role. One file read, no launch.
fn role_type_at(worktree: &Path, path: &str) -> Option<String> {
    let (prefix, suffix) = role_document_affixes();
    let id = path.strip_prefix(&prefix)?.strip_suffix(&suffix)?;
    if id.contains('/') {
        return None;
    }
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(worktree.join(path)).ok()?).ok()?;
    // A brake (review of #344, N3): a value that is not one of the ten is
    // never stamped, so a hand-edited junk type shows as none.
    super::types::of_role(&document)
        .filter(|kind| super::types::get(kind).is_ok())
        .map(str::to_string)
}

/// Every community document at the branch head, with the commit that last
/// touched it: what the store should hold for `project`.
///
/// # Errors
/// [`E_GOV_GIT`] when git cannot list the branch.
pub fn from_branch_head(
    root: &GovernanceRoot,
    project: &str,
) -> Result<Vec<CommunityPointer>, Refusal> {
    let present = read(
        root,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            "HEAD",
            "--",
            COMMUNITY,
        ],
    )?;
    let mut last: BTreeMap<&str, Option<&str>> = present
        .split('\0')
        .filter(|path| is_document(path))
        .map(|path| (path, None))
        .collect();
    let history = read(
        root,
        &[
            "log",
            "-z",
            "--format=commit %H",
            "--name-only",
            "HEAD",
            "--",
            COMMUNITY,
        ],
    )?;
    let mut commit = None;
    for field in history
        .split(['\0', '\n'])
        .filter(|field| !field.is_empty())
    {
        if let Some(sha) = field.strip_prefix("commit ") {
            commit = Some(sha);
        } else if let Some(slot) = last.get_mut(field)
            && slot.is_none()
        {
            *slot = commit;
        }
    }
    Ok(last
        .into_iter()
        .filter_map(|(path, commit)| {
            commit.map(|commit| CommunityPointer {
                project: project.to_string(),
                path: path.to_string(),
                commit: commit.to_string(),
                role_type: role_type_at(&root.worktree, path),
            })
        })
        .collect())
}

/// A file's content at the governance branch head (one git launch).
///
/// # Errors
/// [`E_GOV_GIT`] when git cannot read it.
pub fn read_at_head(root: &GovernanceRoot, path: &str) -> Result<String, Refusal> {
    read(root, &["show", &format!("HEAD:{path}")])
}

fn read(root: &GovernanceRoot, args: &[&str]) -> Result<String, Refusal> {
    let output = git(&root.worktree, args, Locks::Read)
        .map_err(|error| Refusal::new(E_GOV_GIT, format!("could not run git: {error}")))?;
    if !output.status.success() {
        return Err(Refusal::new(
            E_GOV_GIT,
            format!(
                "git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_shipped_type_is_stamped_on_a_role_documents_pointer() {
        let dir = pij_testkit::fresh_dir("pointer-role-type");
        let (prefix, suffix) = role_document_affixes();
        for (id, kind) in [("good", "coder"), ("junk", "wizard")] {
            let path = dir.join(format!("{prefix}{id}{suffix}"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let document = serde_json::json!({"sections": [{"name": "meta", "value": {"id": id, "type": kind}}]});
            std::fs::write(&path, document.to_string()).unwrap();
        }
        assert_eq!(
            role_type_at(&dir, &format!("{prefix}good{suffix}")).as_deref(),
            Some("coder")
        );
        assert_eq!(role_type_at(&dir, &format!("{prefix}junk{suffix}")), None);
        assert_eq!(
            role_type_at(&dir, ".harness/government/community/ROUTES.md"),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::community::ddocs::Validator;
    use crate::community::layout::{RolePaths, rendered};
    use crate::community::test_repo::Scratch;
    use crate::community::writer::{CommunityWriter, Existing, LOCK_RETRY, NewFile};

    struct Renders;

    impl Validator for Renders {
        fn validate(&self, _: &Path, _: &Path) -> Result<(), Refusal> {
            Ok(())
        }
        fn build(&self, _: &Path, document: &Path) -> Result<(), Refusal> {
            std::fs::write(rendered(document), "rendered\n").unwrap();
            Ok(())
        }
    }

    fn role(id: &str, body: &str) -> Vec<NewFile> {
        vec![NewFile {
            path: RolePaths::new(id).unwrap().document,
            bytes: body.as_bytes().to_vec(),
            document: true,
            existing: Existing::Replace,
        }]
    }

    #[tokio::test]
    async fn deleting_the_pointer_rows_and_rebuilding_from_the_branch_head_gives_the_same_rows() {
        let scratch = Scratch::new("pij-community-pointer");
        let writer = CommunityWriter::new(Arc::new(Renders), vec![], &LOCK_RETRY);
        let fresh = pij_testkit::FreshStore::new();
        let store =
            pij_store::SqliteOrchestration::new(pij_store::open(&fresh.path()).await.unwrap());
        for (id, body) in [("pm", "1"), ("qa", "1"), ("pm", "2")] {
            let written = writer
                .write(&scratch.main, role(id, body), "write")
                .unwrap();
            store
                .record_community_pointers(&for_write(
                    "pij",
                    &written.paths,
                    &written.commit,
                    &written.root.worktree,
                ))
                .await
                .unwrap();
        }
        // A commit that touches nothing in community/ moves no pointer.
        std::fs::write(scratch.gov.join("notes.md"), "x\n").unwrap();
        scratch.git(&scratch.gov, &["add", "notes.md"]);
        scratch.git(&scratch.gov, &["commit", "-q", "-m", "unrelated"]);

        let recorded = store.community_pointers("pij").await.unwrap();
        assert_eq!(recorded.len(), 2, "{recorded:?}");
        store.replace_community_pointers("pij", &[]).await.unwrap();
        assert!(store.community_pointers("pij").await.unwrap().is_empty());

        let root = crate::community::root::resolve(&scratch.main).unwrap();
        let rebuilt = from_branch_head(&root, "pij").unwrap();
        store
            .replace_community_pointers("pij", &rebuilt)
            .await
            .unwrap();
        assert_eq!(store.community_pointers("pij").await.unwrap(), recorded);
    }
}
