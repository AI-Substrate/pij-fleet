//! One conversation running as two seats (plan 174, pij-fleet#55).
//!
//! Defined once here because two runtimes spell them: the daemon raises the
//! code and the Claude hook and status-line scripts match it as a literal. The
//! script tests pin each script's literal to [`CONVERSATION_LIVE_CODE`].

/// The refusal every verb gives from a pane running a second copy of a live
/// seat's conversation, and adopt gives for that conversation from any other
/// process. It leads the refusal text and is `details.code`.
pub const CONVERSATION_LIVE_CODE: &str = "E-RS-CONVERSATION-LIVE";

/// The spine record of one detected (live seat, stray process) pair: its audit,
/// and the dedupe record that keeps the live seat's notice to one per pair.
pub const DETECTED_KIND: &str = "seat.double-detected";
