//! The community writer: one per daemon, serialised per governance worktree.
//!
//! Ruling C8. The writer writes into the standing governance worktree and
//! commits with `git add -- <its paths>` then `git commit --only -- <its
//! paths>`, so whatever else is staged, unstaged or untracked there stays
//! exactly as it was, and the checkout's own next commit sees pij's files as
//! committed, never as deleted. No plumbing moves the ref under a checkout.
//!
//! Cost: three git launches to find the root, one status, one add, one commit
//! and one rev-parse per write, plus a ddocs validate and build per document.
//! Nothing runs on a timer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::ddocs::Validator;
use super::git::{Locks, git};
use super::layout::{self, rendered};
use super::root::{self, GovernanceRoot};
use super::{E_FOREIGN_EDIT, E_GOV_GIT, E_GOV_LOCKED, Refusal};

/// How long to wait for a held `index.lock` before refusing: the waits between
/// attempts. Never forced.
pub const LOCK_RETRY: [Duration; 4] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

/// One file the writer puts in the governance worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFile {
    /// Relative to the governance worktree's top level.
    pub path: PathBuf,
    /// The whole content.
    pub bytes: Vec<u8>,
    /// `true` for a `.dd.json`: validated, then rendered to its `.dd.md`.
    pub document: bool,
    /// What an existing file at `path` means.
    pub existing: Existing,
}

/// What a write does when its path already holds a committed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Existing {
    /// Refuse with this refusal (`directory add` of a role that exists).
    Refuse(Refusal),
    /// Overwrite it.
    Replace,
}

/// A landed write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The governance worktree it was committed in.
    pub root: GovernanceRoot,
    /// The new commit on the governance branch.
    pub commit: String,
    /// Every path in the commit, relative to the worktree, sorted.
    pub paths: Vec<String>,
}

/// The daemon's single community writer.
pub struct CommunityWriter {
    validator: Arc<dyn Validator>,
    schemas: Vec<(PathBuf, &'static [u8])>,
    lock_retry: Vec<Duration>,
    worktrees: Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>,
}

impl CommunityWriter {
    /// A writer over `validator`, carrying `schemas` as the copies the
    /// governance branch must hold (ruling C9), waiting `lock_retry` between
    /// attempts on a held index lock.
    pub fn new(
        validator: Arc<dyn Validator>,
        schemas: Vec<(PathBuf, &'static [u8])>,
        lock_retry: &[Duration],
    ) -> Self {
        Self {
            validator,
            schemas,
            lock_retry: lock_retry.to_vec(),
            worktrees: Mutex::new(BTreeMap::new()),
        }
    }

    /// Write `files` into the governance worktree of the repository holding
    /// `repo`, refreshing stale schema copies, and commit only those paths with
    /// `message`. Blocking: call it off the async runtime.
    ///
    /// # Errors
    /// A named [`Refusal`]; on any refusal the worktree and branch are as they
    /// were.
    pub fn write(
        &self,
        repo: &Path,
        files: Vec<NewFile>,
        message: &str,
    ) -> Result<Written, Refusal> {
        let root = root::resolve(repo)?;
        let lock = {
            let mut worktrees = self
                .worktrees
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(worktrees.entry(root.worktree.clone()).or_default())
        };
        let _serial = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.write_serialised(root, files, message)
    }

    fn write_serialised(
        &self,
        root: GovernanceRoot,
        mut files: Vec<NewFile>,
        message: &str,
    ) -> Result<Written, Refusal> {
        let top = root.worktree.clone();
        for (path, bytes) in &self.schemas {
            if std::fs::read(top.join(path)).ok().as_deref() != Some(*bytes) {
                files.push(NewFile {
                    path: path.clone(),
                    bytes: bytes.to_vec(),
                    document: false,
                    existing: Existing::Replace,
                });
            }
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        for file in &files {
            paths.push(file.path.clone());
            if file.document {
                paths.push(rendered(&file.path));
            }
        }
        paths.sort();
        paths.dedup();

        let foreign = foreign_edits(&top, &paths)?;
        if !foreign.is_empty() {
            return Err(Refusal::new(
                E_FOREIGN_EDIT,
                format!(
                    "these paths carry edits pij did not make, so it wrote nothing: {}",
                    foreign.join(", ")
                ),
            )
            .with_next(format!(
                "Commit or remove those edits in {}, then retry.",
                top.display()
            )));
        }
        for file in &files {
            if let Existing::Refuse(refusal) = &file.existing
                && top.join(&file.path).exists()
            {
                return Err(refusal.clone());
            }
        }

        let mut undo = Undo::new(&top, &paths);
        for file in &files {
            let target = top.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| io_refusal(&target, &error))?;
            }
            std::fs::write(&target, &file.bytes).map_err(|error| io_refusal(&target, &error))?;
        }
        for file in files.iter().filter(|file| file.document) {
            self.validator.validate(&top, &top.join(&file.path))?;
        }
        for file in files.iter().filter(|file| file.document) {
            self.validator.build(&top, &top.join(&file.path))?;
        }

        let names: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let mut add = vec!["add".to_string(), "--".to_string()];
        add.extend(names.iter().cloned());
        self.git_write(&top, &add)?;
        undo.staged = true;
        let mut commit = vec![
            "commit".to_string(),
            "--only".to_string(),
            "--quiet".to_string(),
            "--message".to_string(),
            message.to_string(),
            "--".to_string(),
        ];
        commit.extend(names.iter().cloned());
        self.git_write(&top, &commit)?;
        undo.committed = true;
        let head = run(&top, &["rev-parse", "HEAD"], Locks::Read)?;
        Ok(Written {
            root,
            commit: head.trim().to_string(),
            paths: names,
        })
    }

    /// A git write, retried while another process holds the index lock.
    fn git_write(&self, top: &Path, args: &[String]) -> Result<(), Refusal> {
        let mut waits = self.lock_retry.iter();
        loop {
            match run(top, args, Locks::Write) {
                Ok(_) => return Ok(()),
                Err(refusal) if refusal.message.contains("index.lock") => match waits.next() {
                    Some(wait) => std::thread::sleep(*wait),
                    None => {
                        return Err(Refusal::new(
                            E_GOV_LOCKED,
                            format!(
                                "the governance worktree's index stayed locked; pij wrote nothing: {}",
                                refusal.message
                            ),
                        )
                        .with_next(format!(
                            "Let the git command running in {} finish, then retry. pij never removes the lock.",
                            top.display()
                        )));
                    }
                },
                Err(refusal) => return Err(refusal),
            }
        }
    }
}

/// Every path in `paths` that git reports as changed, staged or untracked.
fn foreign_edits(top: &Path, paths: &[PathBuf]) -> Result<Vec<String>, Refusal> {
    let mut args = vec![
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "-z".to_string(),
        "--untracked-files=all".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().map(|path| path.display().to_string()));
    let listing = run(top, &args, Locks::Read)?;
    let mut foreign = Vec::new();
    let mut records = listing.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        let (status, path) = record.split_at(record.len().min(3));
        foreign.push(format!("{} ({})", path, status.trim()));
        // A rename or copy record carries its source path next.
        if status.starts_with(['R', 'C']) {
            records.next();
        }
    }
    Ok(foreign)
}

fn run<S: AsRef<str>>(top: &Path, args: &[S], locks: Locks) -> Result<String, Refusal> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let output = git(top, &args, locks)
        .map_err(|error| Refusal::new(E_GOV_GIT, format!("could not run git: {error}")))?;
    if !output.status.success() {
        return Err(Refusal::new(
            E_GOV_GIT,
            format!(
                "git {} failed in {}: {}",
                args.first().copied().unwrap_or_default(),
                top.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn io_refusal(path: &Path, error: &std::io::Error) -> Refusal {
    Refusal::new(
        E_GOV_GIT,
        format!("could not write {}: {error}", path.display()),
    )
}

/// Puts the worktree back the way the write found it, unless the commit landed.
/// Every path was clean before the write (the foreign-edit check), so its
/// committed content, or its absence, is exactly what was there.
struct Undo {
    top: PathBuf,
    before: Vec<(PathBuf, Option<Vec<u8>>)>,
    staged: bool,
    committed: bool,
}

impl Undo {
    fn new(top: &Path, paths: &[PathBuf]) -> Self {
        Self {
            top: top.to_path_buf(),
            before: paths
                .iter()
                .map(|path| (path.clone(), std::fs::read(top.join(path)).ok()))
                .collect(),
            staged: false,
            committed: false,
        }
    }
}

impl Drop for Undo {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if self.staged {
            let mut args = vec!["reset".to_string(), "--quiet".to_string(), "--".to_string()];
            args.extend(
                self.before
                    .iter()
                    .map(|(path, _)| path.display().to_string()),
            );
            let _ = run(&self.top, &args, Locks::Write);
        }
        for (path, bytes) in &self.before {
            let target = self.top.join(path);
            match bytes {
                Some(bytes) => {
                    let _ = std::fs::write(&target, bytes);
                }
                None => {
                    let _ = std::fs::remove_file(&target);
                    // Drop directories the write created, up to the community root.
                    let mut dir = target.parent();
                    while let Some(current) = dir {
                        if current == self.top.join(layout::COMMUNITY)
                            || current == self.top
                            || std::fs::remove_dir(current).is_err()
                        {
                            break;
                        }
                        dir = current.parent();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
