//! Routing-table delivery (plan 202, roadmap 001 s08).
//!
//! A seat gets the table as one held FYI from `pij-bg` when it is assigned a
//! role, after a compact, and when a revive brings it back; the prime gets one
//! held FYI naming the rows a directory write changed. Held, never woken: the
//! table is context for the seat's next turn, not a task. A failed delivery
//! never fails the assign, compact or revive that triggered it.
//!
//! Cost: a few store reads, an in-process read of the role documents and one
//! held FYI per trigger. No launch, no timer.

use std::collections::BTreeMap;

use pij_core::BG_ACTOR;
use pij_core::model::{Msg, SeatId};
use pij_core::orchestration::{PrimeState, Project};
use serde_json::{Value, json};

use crate::Services;
use crate::community::routes::{self, Change, Table};
use crate::community::{Refusal, root};

/// What brought the table to a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// `directory assign` landed.
    Assign,
    /// A `compact` control was admitted.
    Compact,
    /// A revive relaunched the seat.
    Revive,
}

/// The project's table and the copy a seat is sent, or `None` when the
/// project has no governance worktree or no roles. Launches nothing.
///
/// # Errors
/// `E-RS-STORE` when the holders cannot be read.
pub(crate) async fn table_for(
    services: &Services,
    project: &Project,
) -> Result<Option<(Table, String)>, Refusal> {
    let Some(repo) = project.repo.as_deref() else {
        return Ok(None);
    };
    let Ok(root) = root::find(std::path::Path::new(repo)) else {
        return Ok(None);
    };
    let roles = routes::read_roles(&root.worktree);
    if roles.is_empty() {
        return Ok(None);
    }
    let table = routes::build(&roles);
    let text = routes::for_seat(
        &project.slug,
        &table,
        &live_holders(services, &project.slug).await?,
    );
    Ok(Some((table, text)))
}

/// Each role's live holders in `project`, as `seat` or `seat (instance)`.
///
/// # Errors
/// `E-RS-STORE`.
pub(crate) async fn live_holders(
    services: &Services,
    project: &str,
) -> Result<BTreeMap<String, Vec<String>>, Refusal> {
    let rows = services
        .directory
        .store()
        .assignments(project)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    let mut holders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows.into_iter().filter(|row| row.to.is_none()) {
        let seat = if row.instance.is_empty() {
            row.seat
        } else {
            format!("{} ({})", row.seat, row.instance)
        };
        holders.entry(row.role).or_default().push(seat);
    }
    Ok(holders)
}

/// Hold the table for `seat`: for `project` when named, else for every
/// project the seat holds a role in, else (revive only) the project whose
/// repo holds the seat's folder. One FYI however many projects. Never fails;
/// the returned value says what happened.
pub(crate) async fn deliver(
    services: &Services,
    seat: &SeatId,
    project: Option<&str>,
    trigger: Trigger,
) -> Value {
    match try_deliver(services, seat, project, trigger).await {
        Ok(outcome) => outcome,
        Err(refusal) => {
            eprintln!("pij-rs could not send {seat} the routing table ({trigger:?}): {refusal}");
            json!({"sent": false, "reason": refusal.to_string()})
        }
    }
}

async fn try_deliver(
    services: &Services,
    seat: &SeatId,
    project: Option<&str>,
    trigger: Trigger,
) -> Result<Value, Refusal> {
    let store = services.directory.store();
    let projects = store
        .list_projects()
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    let mut slugs: Vec<String> = match project {
        Some(project) => vec![project.to_string()],
        None => store
            .open_assignments(seat.as_str())
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
            .into_iter()
            .map(|row| row.project)
            .collect(),
    };
    slugs.sort();
    slugs.dedup();
    if slugs.is_empty() && trigger == Trigger::Revive {
        let folder = services
            .registry
            .get(seat)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
            .map(|descriptor| descriptor.folder);
        if let Some(folder) = folder
            && let Ok(found) =
                super::directory::match_project(projects.clone(), std::path::Path::new(&folder))
        {
            slugs.push(found.slug);
        }
    }
    let mut sent = Vec::new();
    let mut texts = Vec::new();
    for slug in &slugs {
        let Some(project) = projects.iter().find(|project| &project.slug == slug) else {
            continue;
        };
        if let Some((_, text)) = table_for(services, project).await? {
            sent.push(slug.clone());
            texts.push(text);
        }
    }
    if texts.is_empty() {
        return Ok(json!({"sent": false, "reason": "the seat is in no project with roles"}));
    }
    let receipt = hold(services, seat, texts.join("\n\n")).await?;
    Ok(json!({"sent": "fyi", "msg_id": receipt, "projects": sent}))
}

/// Hold one FYI naming `change` for the project's prime: the project's own
/// prime, else the machine's current designated prime. Nothing when no row
/// changed, or there is no live prime. Never fails.
pub(crate) async fn notify_prime(
    services: &Services,
    project: &str,
    commit: &str,
    change: &Change,
) -> Value {
    if change.is_empty() {
        return json!({"sent": false, "reason": "no route changed"});
    }
    match try_notify_prime(services, project, commit, change).await {
        Ok(outcome) => outcome,
        Err(refusal) => {
            eprintln!("pij-rs could not tell the prime of {project} its routes changed: {refusal}");
            json!({"sent": false, "reason": refusal.to_string()})
        }
    }
}

async fn try_notify_prime(
    services: &Services,
    project: &str,
    commit: &str,
    change: &Change,
) -> Result<Value, Refusal> {
    let store = services.directory.store();
    let own = store
        .list_projects()
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        .into_iter()
        .find(|known| known.slug == project)
        .and_then(|known| known.prime_id);
    let prime = match own {
        Some(prime) => Some(prime),
        None => store
            .prime()
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
            .filter(|designation| designation.state == PrimeState::Current)
            .map(|designation| designation.seat),
    };
    let Some(prime) = prime else {
        return Ok(json!({"sent": false, "reason": "the project has no prime"}));
    };
    let live = services
        .registry
        .get(&prime)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        .is_some_and(|seat| seat.tombstoned_at.is_none());
    if !live {
        return Ok(json!({"sent": false, "prime": prime, "reason": "the prime is not live"}));
    }
    let msg_id = hold(
        services,
        &prime,
        routes::change_notice(project, commit, change),
    )
    .await?;
    Ok(json!({"sent": "fyi", "prime": prime, "msg_id": msg_id}))
}

async fn hold(services: &Services, to: &SeatId, body: String) -> Result<String, Refusal> {
    let msg_id = services.delivery.next_message_id();
    services
        .delivery
        .hold_fyi(Msg {
            from: SeatId::from(BG_ACTOR),
            to: to.clone(),
            body,
            msg_id: msg_id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        })
        .await
        .map_err(|error| Refusal::new("E-RS-DELIVERY", error.to_string()))?;
    Ok(msg_id)
}
