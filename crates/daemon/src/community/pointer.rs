//! Rebuild the store's community pointers from the governance branch head
//! (plan 190, AC7). Two git launches, on demand only.

use std::collections::BTreeMap;

use pij_store::CommunityPointer;

use super::git::{Locks, git};
use super::layout::{COMMUNITY, is_document};
use super::root::GovernanceRoot;
use super::{E_GOV_GIT, Refusal};

/// The pointers a write records: one per community document it committed.
pub fn for_write(project: &str, paths: &[String], commit: &str) -> Vec<CommunityPointer> {
    paths
        .iter()
        .filter(|path| is_document(path))
        .map(|path| CommunityPointer {
            project: project.to_string(),
            path: path.clone(),
            commit: commit.to_string(),
        })
        .collect()
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
                .record_community_pointers(&for_write("pij", &written.paths, &written.commit))
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
