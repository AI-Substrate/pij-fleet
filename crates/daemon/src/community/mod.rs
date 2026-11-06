//! The community directory's writer (plan 190, roadmap 001 step s02).
//!
//! Role definitions live under `<government-root>/community/` on the
//! `prime-governance` branch. The daemon is their only writer: it writes the
//! document, has ddocs validate and render it, and commits only the paths it
//! wrote, inside the standing governance worktree (ruling C8). Every refusal is
//! named, with the step that fixes it.

pub mod ddocs;
pub mod directory;
pub mod gh;
mod git;
pub mod holders;
pub mod layout;
pub mod memory;
pub mod output;
pub mod picture;
pub mod pointer;
pub mod role;
pub mod root;
pub mod templates;
pub mod writer;

use serde::Serialize;

/// A named refusal: a stable code, what went wrong, and the step that fixes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    /// Stable `E-RS-*` code.
    pub code: &'static str,
    /// What went wrong, in words.
    pub message: String,
    /// A runnable next step, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl Refusal {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            next: None,
        }
    }

    pub(crate) fn with_next(mut self, next: impl Into<String>) -> Self {
        self.next = Some(next.into());
        self
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.code, self.message)?;
        if let Some(next) = &self.next {
            write!(f, "\n{next}")?;
        }
        Ok(())
    }
}

/// No standing governance worktree, or it is unusable.
pub const E_GOV_ABSENT: &str = "E-RS-GOV-ABSENT";
/// ddocs is not installed, or not executable.
pub const E_DDOCS_MISSING: &str = "E-RS-DDOCS-MISSING";
/// ddocs rejected the document.
pub const E_DDOCS_INVALID: &str = "E-RS-DDOCS-INVALID";
/// A path the writer is about to write carries an edit it did not make.
pub const E_FOREIGN_EDIT: &str = "E-RS-GOV-FOREIGN-EDIT";
/// The governance worktree's index stayed locked past the retry window.
pub const E_GOV_LOCKED: &str = "E-RS-GOV-LOCKED";
/// A destination, or a directory above it, is a symlink.
pub const E_GOV_SYMLINK: &str = "E-RS-GOV-SYMLINK";
/// A git command the writer needs failed.
pub const E_GOV_GIT: &str = "E-RS-GOV-GIT";
/// A role's standing packet names a brief that is not a readable file in the
/// governance worktree.
pub const E_BRIEF_MISSING: &str = "E-RS-ROLE-BRIEF-MISSING";
/// `directory add` named a role that already exists.
pub const E_DIRECTORY_EXISTS: &str = "E-RS-DIRECTORY-EXISTS";

#[cfg(test)]
pub(crate) mod test_repo;
