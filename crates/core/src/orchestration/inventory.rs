use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Ordinals observed in each Git namespace that can reserve one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoInventory {
    /// The primary clone's checked-out name/ref.
    pub clone: BTreeSet<u32>,
    /// Linked worktree directory or branch names.
    pub worktrees: BTreeSet<u32>,
    /// Local `refs/heads` names, including branches without worktrees.
    pub branch_heads: BTreeSet<u32>,
}

impl RepoInventory {
    /// Every reserved ordinal, deduplicated across namespaces.
    pub fn all_ordinals(&self) -> BTreeSet<u32> {
        self.clone
            .iter()
            .chain(&self.worktrees)
            .chain(&self.branch_heads)
            .copied()
            .collect()
    }
}

/// A pure stream-creation recipe for the daemon's Git adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPlan {
    /// Project slug.
    pub project: String,
    /// Stream slug.
    pub slug: String,
    /// Reserved ordinal.
    pub ordinal: u32,
    /// Branch to create.
    pub branch: String,
    /// Absolute worktree path.
    pub worktree: PathBuf,
    /// Base ref the caller selected.
    pub base_ref: String,
}

/// Why a stream recipe cannot be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamPlanError {
    /// Project or stream slug is not lowercase kebab case.
    InvalidSlug {
        /// Rejected value.
        value: String,
    },
    /// No higher ordinal can be represented.
    OrdinalExhausted,
    /// The requested ordinal is already present in a Git namespace.
    OrdinalReserved {
        /// Conflicting ordinal.
        ordinal: u32,
    },
    /// Worktree root was relative; cross-tree writes require an absolute root.
    RelativeWorktreeRoot,
}

/// Mint one greater than the highest ordinal in clone, worktrees, or heads.
pub fn mint_ordinal(inventory: &RepoInventory) -> Result<u32, StreamPlanError> {
    inventory
        .all_ordinals()
        .last()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(StreamPlanError::OrdinalExhausted)
}

/// Build a stream recipe without consulting source-tree dirt.
///
/// Dirty state is intentionally absent from the input. `git worktree add`
/// creates another checkout from a ref and does not need the primary checkout's
/// index to be clean; requiring cleanliness was TS defect #10.
pub fn plan_stream_creation(
    project: &str,
    slug: &str,
    worktree_root: &Path,
    base_ref: &str,
    inventory: &RepoInventory,
) -> Result<StreamPlan, StreamPlanError> {
    stream_plan(project, slug, worktree_root, base_ref, inventory, None)
}

/// Plan an explicitly numbered stream using the same validation and naming rules.
/// Inventory collisions refuse without mutating the caller's observed inventory;
/// the durable store reservation remains the final concurrent-allocation guard.
pub fn plan_stream_creation_at_ordinal(
    project: &str,
    slug: &str,
    worktree_root: &Path,
    base_ref: &str,
    inventory: &RepoInventory,
    ordinal: u32,
) -> Result<StreamPlan, StreamPlanError> {
    stream_plan(
        project,
        slug,
        worktree_root,
        base_ref,
        inventory,
        Some(ordinal),
    )
}

fn stream_plan(
    project: &str,
    slug: &str,
    worktree_root: &Path,
    base_ref: &str,
    inventory: &RepoInventory,
    ordinal: Option<u32>,
) -> Result<StreamPlan, StreamPlanError> {
    for value in [project, slug] {
        if !valid_slug(value) {
            return Err(StreamPlanError::InvalidSlug {
                value: value.to_string(),
            });
        }
    }
    if !worktree_root.is_absolute() {
        return Err(StreamPlanError::RelativeWorktreeRoot);
    }
    let ordinal = match ordinal {
        Some(ordinal)
            if inventory.clone.contains(&ordinal)
                || inventory.worktrees.contains(&ordinal)
                || inventory.branch_heads.contains(&ordinal) =>
        {
            return Err(StreamPlanError::OrdinalReserved { ordinal });
        }
        Some(ordinal) => ordinal,
        None => mint_ordinal(inventory)?,
    };
    let padded = format!("{ordinal:03}");
    Ok(StreamPlan {
        project: project.to_string(),
        slug: slug.to_string(),
        ordinal,
        branch: format!("s{padded}/{slug}"),
        worktree: worktree_root.join(format!("s{padded}-{slug}")),
        base_ref: base_ref.to_string(),
    })
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
