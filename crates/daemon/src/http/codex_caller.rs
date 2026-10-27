//! A Codex caller's claimed seat must be the one its own thread runs in
//! (plan 181).
//!
//! A shared Codex app-server gives every thread it hosts the environment of the
//! session that started it, so a Codex tool shell's `PIJ_SESSION_ID` and
//! `TMUX_PANE` can both name another seat — consistently, so the ordinary
//! pane-versus-id disagreement check never fires. Its `CODEX_THREAD_ID` is
//! always its own. This check holds the claim to that thread: the thread the
//! seat records, or else the thread the Codex process in the claimed pane
//! holds. When neither confirms it, the caller cannot derive its identity, and
//! the answer is a refusal that names why. It never advises `pij adopt`:
//! adopting the inherited pane is how a Codex seat was once registered as a
//! stranger with no parent.

use pij_core::error::Result;
use pij_core::model::{Harness, SeatDescriptor};

use axum::response::Response;

use super::identity::CallerContext;
use super::{AppState, internal, refused};

/// Why a Codex caller is not the seat it resolved to, or the `pane` it claims
/// when no seat resolved; `None` when it is, or when the caller is not inside
/// Codex, or the seat is not a Codex seat.
pub(crate) async fn contradiction(
    state: &AppState,
    caller: Option<&CallerContext>,
    seat: Option<&SeatDescriptor>,
    pane: Option<&str>,
) -> Result<Option<String>> {
    let Some(thread) = caller
        .and_then(|caller| caller.codex_session.as_deref())
        .filter(|thread| !thread.is_empty())
    else {
        return Ok(None);
    };
    if seat.is_some_and(|seat| seat.harness != Harness::Codex) {
        return Ok(None);
    }
    let claimed = seat.map_or_else(
        || pane.map(|pane| format!("pane `{pane}`")),
        |seat| Some(format!("seat `{}`", seat.id)),
    );
    let Some(claimed) = claimed else {
        return Ok(None);
    };
    if let Some(recorded) = seat.and_then(|seat| seat.harness_session.as_deref()) {
        return Ok((recorded != thread).then(|| {
            refusal(
                thread,
                &claimed,
                &format!("that seat records thread `{recorded}`"),
            )
        }));
    }
    let Some(pane) = seat.map_or(pane, |seat| seat.pane.as_deref()) else {
        return Ok(None);
    };
    let evidence = match state.services.codex_threads.held_by_pane(pane).await? {
        Some(held) if held.thread == thread => return Ok(None),
        Some(held) => format!(
            "the Codex process in pane `{pane}` holds thread `{}`",
            held.thread
        ),
        None => format!("no Codex process in pane `{pane}` holds a thread pij can read"),
    };
    Ok(Some(refusal(thread, &claimed, &evidence)))
}

fn refusal(thread: &str, claimed: &str, evidence: &str) -> String {
    format!(
        "E-RS-CODEX-IDENTITY: this Codex thread `{thread}` claims {claimed}, but {evidence}. A \
         shared Codex app-server gives every thread the environment of the session that \
         started it, so PIJ_SESSION_ID and TMUX_PANE can name another seat. Refusing rather \
         than acting as that seat; nothing was written."
    )
}

/// [`contradiction`] as the refusal a route returns, or `None` to carry on.
pub(crate) async fn refusal_for(
    state: &AppState,
    command: &'static str,
    caller: Option<&CallerContext>,
    seat: Option<&SeatDescriptor>,
    pane: Option<&str>,
) -> Option<Response> {
    match contradiction(state, caller, seat, pane).await {
        Ok(None) => None,
        Ok(Some(reason)) => Some(refused(command, reason)),
        Err(error) => Some(internal(command, error)),
    }
}
