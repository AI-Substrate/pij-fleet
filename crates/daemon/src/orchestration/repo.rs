//! Git-backed repository inventory and stream worktree creation.
//!
//! This module gathers IO facts for the pure `pij-core` decisions. It is kept
//! out of the functional core because process execution and filesystem traversal
//! are daemon responsibilities.

use std::path::Path;
use std::process::Command;

use pij_core::error::{PijError, Result};
use pij_core::model::SeatId;
use pij_core::orchestration::{RepoInventory, StreamPlan};
use pij_store::{SqliteOrchestration, StreamReservation};

/// Outcome of reserving and materialising a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamCreation {
    /// Reservation persisted and `git worktree add` completed.
    Created,
    /// Another writer already reserved the allocation; Git was not invoked.
    AlreadyReserved,
}

/// Scan ordinals from the clone, every linked worktree, and every local head.
pub fn scan_repo_inventory(repo_root: &Path) -> Result<RepoInventory> {
    if !repo_root.is_absolute() {
        return Err(adapter_failure(
            "repository root must be absolute; relative roots can scan the wrong checkout",
        ));
    }
    let mut inventory = RepoInventory::default();

    if let Some(name) = repo_root.file_name().and_then(|name| name.to_str())
        && let Some(ordinal) = parse_ordinal(name)
    {
        inventory.clone.insert(ordinal);
    }
    let current_branch = git(repo_root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if let Some(ordinal) = parse_ordinal(current_branch.trim()) {
        inventory.clone.insert(ordinal);
    }

    let worktrees = git(repo_root, &["worktree", "list", "--porcelain"])?;
    for line in worktrees.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str())
                && let Some(ordinal) = parse_ordinal(name)
            {
                inventory.worktrees.insert(ordinal);
            }
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/")
            && let Some(ordinal) = parse_ordinal(branch)
        {
            inventory.worktrees.insert(ordinal);
        }
    }

    let heads = git(
        repo_root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    for head in heads.lines() {
        if let Some(ordinal) = parse_ordinal(head.trim()) {
            inventory.branch_heads.insert(ordinal);
        }
    }
    Ok(inventory)
}

/// Persist a stream reservation before creating its Git worktree.
///
/// Deliberately never runs `git status`: source-tree dirt cannot affect a
/// worktree created from `plan.base_ref` (TS defect #10).
pub async fn reserve_and_create_stream(
    repo_root: &Path,
    store: &SqliteOrchestration,
    plan: &StreamPlan,
    actor: &SeatId,
    at: u64,
) -> Result<StreamCreation> {
    match store.reserve_stream(plan, actor, at).await? {
        StreamReservation::Conflict => return Ok(StreamCreation::AlreadyReserved),
        StreamReservation::Reserved => {}
    }
    let path = plan.worktree.to_str().ok_or_else(|| {
        adapter_failure("stream worktree path is not valid UTF-8; Git cannot receive it")
    })?;
    git(
        repo_root,
        &["worktree", "add", "-b", &plan.branch, path, &plan.base_ref],
    )?;
    Ok(StreamCreation::Created)
}

fn parse_ordinal(value: &str) -> Option<u32> {
    let value = value.strip_prefix('s')?;
    let digit_count = value.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let (digits, suffix) = value.split_at(digit_count);
    if !suffix.starts_with('-') && !suffix.starts_with('/') {
        return None;
    }
    digits.parse().ok()
}

fn git(repo_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|error| adapter_failure(format!("could not execute git: {error}")))?;
    if !output.status.success() {
        return Err(adapter_failure(format!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| adapter_failure(format!("git emitted non-UTF-8 output: {error}")))
}

fn adapter_failure(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/orchestration/repo".to_string(),
        message: message.into(),
    }
}
