use std::ffi::OsString;
use std::fmt;

use pij_cli::{DaemonClient, IdentityRequest};
use pij_core::model::SeatId;

/// A refusal whose text names the unusable identity input.
#[derive(Debug)]
pub(crate) struct NamedRefusal {
    message: String,
}

impl NamedRefusal {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for NamedRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Validate and forward the environment's identity claims without resolving them.
pub(crate) fn request_from_environment(
    env: Option<OsString>,
    pane: Option<OsString>,
) -> Result<IdentityRequest, NamedRefusal> {
    let seat = decode("PIJ_SESSION_ID", env)?;
    let pane = decode("TMUX_PANE", pane)?;
    if seat.is_none() && pane.is_none() {
        return Err(NamedRefusal::new(
            "no acting seat: set PIJ_SESSION_ID or run from a registered tmux pane (TMUX_PANE); --from/--seat may assert agreement but cannot create identity",
        ));
    }
    Ok(IdentityRequest {
        seat,
        pane,
        ..IdentityRequest::default()
    })
}

/// Resolve the acting seat through the daemon's single identity authority.
///
/// Send every available identity input together. The daemon treats an observable
/// pane as authoritative and refuses a contradictory `PIJ_SESSION_ID`; without
/// a pane, the asserted environment id remains the only available evidence.
pub(crate) async fn resolve_acting_seat(
    env: Option<OsString>,
    pane: Option<OsString>,
    client: &DaemonClient,
) -> Result<SeatId, NamedRefusal> {
    let request = request_from_environment(env, pane)?;

    let response = client.whoami(&request).await;
    if !response.ok {
        return Err(NamedRefusal::new(response.meta.unwrap_or_else(|| {
            "the daemon refused to resolve the acting seat without a reason".to_string()
        })));
    }
    response
        .data
        .map(|seat| seat.id)
        .ok_or_else(|| NamedRefusal::new("the daemon returned no acting seat"))
}

fn decode(name: &str, value: Option<OsString>) -> Result<Option<String>, NamedRefusal> {
    value
        .map(|value| {
            value
                .into_string()
                .map_err(|_| NamedRefusal::new(format!("{name} is not valid UTF-8")))
                .map(|value| value.trim().to_string())
        })
        .transpose()
        .map(|value| value.filter(|value| !value.is_empty()))
}
