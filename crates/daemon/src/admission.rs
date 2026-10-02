//! The two admission consumers, and the proof they cannot disagree.
//!
//! Discovery (panes the daemon finds) and registration (processes that ask to
//! join) are the two places TS answered the admission question separately, and
//! the drift between them is how subagents became seats. Both are thin here on
//! purpose: each maps its own observation into a `Candidate` and then calls the
//! ONE decision. Neither is allowed a rule of its own.

use pij_core::admission::{Admission, Candidate, admit};
use pij_core::model::Pane;

/// Discovery's consumer: what the sweep saw in a tmux pane.
///
/// The sweep knows the pane and the folder; whether a live session is bound is
/// something it must have verified, never assumed from the pane existing.
pub fn from_pane(
    pane: &Pane,
    harness: Option<&str>,
    folder: Option<&str>,
    bound: bool,
) -> Admission {
    admit(&Candidate {
        id: pane.title.clone(),
        harness: harness.map(str::to_string),
        pane: Some(pane.id.clone()),
        folder: folder.map(str::to_string),
        bind_evidence: bound,
        subagent_of: None,
    })
}

/// Registration's consumer: what a process claimed about itself.
///
/// Everything here is self-reported, which is exactly why it goes through the
/// same decision: a claim is not evidence, and `PIJ_PARENT_ID` in a child's
/// environment is a claim.
pub fn from_registration(
    id: &str,
    harness: Option<&str>,
    folder: Option<&str>,
    bound: bool,
    subagent_of: Option<&str>,
) -> Admission {
    admit(&Candidate {
        id: id.to_string(),
        harness: harness.map(str::to_string),
        pane: None,
        folder: folder.map(str::to_string),
        bind_evidence: bound,
        subagent_of: subagent_of.map(str::to_string),
    })
}
