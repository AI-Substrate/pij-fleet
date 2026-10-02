//! Admission: is this observed thing a seat we should register?
//!
//! ONE component, two consumers (workshop 001 R6f). The daemon's discovery sweep
//! asks it about panes it finds; the registration guard asks it about processes
//! that ask to join. Those two answered the question separately in TS and drifted,
//! which is how subagents inheriting `PIJ_*` environment variables registered as
//! seats (defect #3, five duplicate descriptors in one phase) — the sweep would
//! never have admitted them, but registration never asked the sweep's question.
//!
//! The rule is not "share some helpers": it is **one function, one case table,
//! and a gate-enforced test that both consumers answer every case identically**.
//! A second implementation of this decision is a defect, not an optimisation.

use crate::model::Harness;

/// What we know about something that might be a seat.
///
/// Deliberately not a `SeatDescriptor`: a candidate is what an OBSERVER saw, and
/// half of these fields are exactly the ones that turn out to be missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The id it claims.
    pub id: String,
    /// The harness string as observed — unparsed, because "unrecognised harness"
    /// is one of the verdicts.
    pub harness: Option<String>,
    /// The tmux pane it claims, if any. `None` is legitimate (external pull mode).
    pub pane: Option<String>,
    /// Absolute path of the folder it claims to work in.
    pub folder: Option<String>,
    /// Whether the observer verified a live process bound to this candidate.
    ///
    /// This is the field defect #3 turns on: a subagent inherits its parent's
    /// environment and can CLAIM anything, but it cannot manufacture bind
    /// evidence for a session that is not its own.
    pub bind_evidence: bool,
    /// Whether the candidate is a child process of an existing seat rather than a
    /// seat in its own right.
    pub subagent_of: Option<String>,
}

impl Candidate {
    /// A candidate with nothing established — the honest starting point for an
    /// observer that has only seen a name.
    pub fn claiming(id: &str) -> Self {
        Candidate {
            id: id.to_string(),
            harness: None,
            pane: None,
            folder: None,
            bind_evidence: false,
            subagent_of: None,
        }
    }
}

/// The verdict, with its reason attached — a refusal that does not say why turns
/// into a support conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Register it.
    Admit,
    /// Do not register it, because:
    Refuse(RefusalReason),
}

/// Why a candidate was refused. Each variant is a distinct real incident, kept
/// separate so an operator can tell "not ready yet" from "never will be".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    /// No id at all.
    NoId,
    /// The harness string is missing or unrecognised. Never guessed: a wrong
    /// harness binds the seat to the wrong readiness anchor.
    UnknownHarness(String),
    /// No absolute folder. A seat with no folder resolves relative paths against
    /// whatever the daemon's cwd happens to be — the cross-tree write class.
    NoAbsoluteFolder,
    /// It claims a seat identity but nothing proves a live session belongs to it.
    NoBindEvidence,
    /// It is a child of an existing seat. Subagents are not seats; they inherit
    /// the environment that makes them look like one.
    Subagent {
        /// The parent it belongs to.
        parent: String,
    },
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefusalReason::NoId => f.write_str(
                "no seat id — nothing can address, supervise or reap a seat that has no name, \
                 so an unnamed candidate is refused rather than given a generated one",
            ),
            RefusalReason::UnknownHarness(observed) => write!(
                f,
                "unrecognised harness `{observed}` — refusing rather than guessing, because the \
                 wrong harness binds the seat to the wrong readiness anchor"
            ),
            RefusalReason::NoAbsoluteFolder => f.write_str(
                "no absolute folder — a seat without one resolves relative paths against the \
                 daemon's cwd, which is how a write lands in the wrong tree",
            ),
            RefusalReason::NoBindEvidence => f.write_str(
                "no bind evidence — an id can be claimed from an inherited environment, but a \
                 live session cannot",
            ),
            RefusalReason::Subagent { parent } => write!(
                f,
                "a subagent of {parent}, not a seat: it inherited PIJ_* from its parent"
            ),
        }
    }
}

/// The one admission decision in this workspace.
///
/// Order matters and is part of the contract: the cheapest, most certain
/// refusals come first, so the reason an operator sees is the most actionable
/// one rather than whichever check happened to run.
pub fn admit(candidate: &Candidate) -> Admission {
    if candidate.id.trim().is_empty() {
        return Admission::Refuse(RefusalReason::NoId);
    }

    if let Some(parent) = &candidate.subagent_of {
        return Admission::Refuse(RefusalReason::Subagent {
            parent: parent.clone(),
        });
    }

    let harness = candidate.harness.as_deref().unwrap_or("");
    if Harness::parse(harness).is_none() {
        return Admission::Refuse(RefusalReason::UnknownHarness(harness.to_string()));
    }

    match candidate.folder.as_deref() {
        Some(folder) if folder.starts_with('/') => {}
        _ => return Admission::Refuse(RefusalReason::NoAbsoluteFolder),
    }

    if !candidate.bind_evidence {
        return Admission::Refuse(RefusalReason::NoBindEvidence);
    }

    Admission::Admit
}

/// The shared case table both consumers must answer identically.
///
/// Lives beside the decision, not in a test file, so a consumer written in a
/// later wave can run it without depending on another crate's tests — and so
/// adding a case is visibly a change to the CONTRACT.
pub fn case_table() -> Vec<(&'static str, Candidate, Admission)> {
    let seat = |id: &str| Candidate {
        id: id.to_string(),
        harness: Some("pi".to_string()),
        pane: Some("%1".to_string()),
        folder: Some("/abs/tree".to_string()),
        bind_evidence: true,
        subagent_of: None,
    };

    vec![
        ("a fully evidenced seat", seat("pij-good"), Admission::Admit),
        (
            "paneless is fine — external pull mode has no pane",
            Candidate {
                pane: None,
                ..seat("pij-paneless")
            },
            Admission::Admit,
        ),
        (
            "empty id",
            Candidate::claiming("   "),
            Admission::Refuse(RefusalReason::NoId),
        ),
        (
            "a subagent that inherited PIJ_* (TS defect #3)",
            Candidate {
                subagent_of: Some("pij-parent".to_string()),
                ..seat("pij-child")
            },
            Admission::Refuse(RefusalReason::Subagent {
                parent: "pij-parent".to_string(),
            }),
        ),
        (
            "unknown harness is refused, never guessed",
            Candidate {
                harness: Some("emacs".to_string()),
                ..seat("pij-odd")
            },
            Admission::Refuse(RefusalReason::UnknownHarness("emacs".to_string())),
        ),
        (
            "missing harness is the same refusal, with an empty observation",
            Candidate {
                harness: None,
                ..seat("pij-nohar")
            },
            Admission::Refuse(RefusalReason::UnknownHarness(String::new())),
        ),
        (
            "a relative folder is refused",
            Candidate {
                folder: Some("relative/path".to_string()),
                ..seat("pij-rel")
            },
            Admission::Refuse(RefusalReason::NoAbsoluteFolder),
        ),
        (
            "no folder at all",
            Candidate {
                folder: None,
                ..seat("pij-nofolder")
            },
            Admission::Refuse(RefusalReason::NoAbsoluteFolder),
        ),
        (
            "claims everything but proves no live session",
            Candidate {
                bind_evidence: false,
                ..seat("pij-claimant")
            },
            Admission::Refuse(RefusalReason::NoBindEvidence),
        ),
        (
            "a subagent that ALSO has bind evidence is still not a seat",
            Candidate {
                subagent_of: Some("pij-parent".to_string()),
                bind_evidence: true,
                ..seat("pij-child2")
            },
            Admission::Refuse(RefusalReason::Subagent {
                parent: "pij-parent".to_string(),
            }),
        ),
    ]
}
