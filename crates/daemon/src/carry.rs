//! F190: a Copilot `/clear` (or `/new`) starts a new native session in the
//! same process, and registration gives it a new seat id, retiring the old
//! one ("superseded by `X` at a native session boundary (same process)").
//! Its open role holdings, the delivered requests it has not answered, and
//! the FYIs held for it stayed on the old id. Here the holdings and FYIs are
//! carried to the successor, and the requests go back on their role's queue
//! so the successor's drain delivers them again: the cleared session never
//! saw them, and the time limit's clock (F73) starts at their new arrival.
//!
//! POLICY, and what decides it: a carry starts only for the supersede the
//! registration that wrote it reports (`to` superseded `from` at a native
//! session boundary in the same process, and both name the same launch:
//! `spawn_id`, both present), and it first records that intent
//! (`seat.carry.started` on `from`). Every redo, at a later registration or
//! at daemon start, finishes only a carry whose intent is recorded: the
//! intent decides, so a boundary from before this code existed is never
//! carried. BRAKES, each of which can only spare: no `spawn_id` on either
//! side, a different one, a successor no longer live, no recorded intent on
//! a redo, or a carry already recorded done (`seat.carried` on `from` with
//! nothing delivering). The store's mail transaction checks the same
//! boundary again in its own read of the seats table and refuses otherwise.
//!
//! The two stores cannot share a commit. Governance first: per project, one
//! holders commit ends `from`'s holdings and opens the same for `to`
//! ([`DirectoryService::carry_project`]); then one SQLite transaction puts the
//! requests back on the queue, moves the FYIs and records `seat.carried`
//! ([`pij_store::SqliteOrchestration::carry_mail_committed`]); then the
//! successor's drain. A crash between any two heals: every step checks
//! state, so a rerun neither duplicates a holding nor moves mail twice; the
//! successor's next registration and the next daemon start redo any carry
//! not recorded done, and drain again. A project whose commit is refused
//! keeps its holdings and its requests on the old seat (recorded in the
//! event); only the seat-addressed FYIs move.
//!
//! Cost: per `/clear` only (a human action): one holders commit per project
//! the old seat held roles in, one SQLite transaction, then one drain; a
//! request still `delivering` is waited for at most 20 × 250 ms. Nothing per
//! tick. At daemon start, one roster read and one spine read per eligible
//! tombstone, once.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use pij_core::model::{SeatDescriptor, SeatId};
use serde_json::json;

use crate::Services;

/// The tombstone reason registration writes when `successor` supersedes a
/// seat at a native session boundary in the same process.
pub(crate) fn boundary_reason(successor: &SeatId) -> String {
    pij_store::native_boundary_reason(successor.as_str())
}

/// The successor a native-session-boundary tombstone names, if `reason` is one.
pub(crate) fn boundary_successor(reason: &str) -> Option<SeatId> {
    reason
        .strip_prefix("superseded by `")?
        .strip_suffix("` at a native session boundary (same process)")
        .filter(|id| !id.is_empty() && !id.contains('`'))
        .map(|id| SeatId::from(id.to_string()))
}

/// The guard (the policy, with its brakes): `to` superseded `from` at a
/// native session boundary in the same process, and both name the same
/// launch. `to` must be live.
pub(crate) fn eligible(from: &SeatDescriptor, to: &SeatDescriptor) -> bool {
    to.tombstoned_at.is_none()
        && from.spawn_id.is_some()
        && from.spawn_id == to.spawn_id
        && from.tombstone_reason.as_deref() == Some(boundary_reason(&to.id).as_str())
}

/// One carry at a time, and the old seats already carried since the daemon
/// started (so a successor's later registrations read nothing).
#[derive(Default)]
pub struct Carries {
    lock: tokio::sync::Mutex<()>,
    done: std::sync::Mutex<HashSet<SeatId>>,
}

impl Carries {
    fn is_done(&self, from: &SeatId) -> bool {
        self.done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(from)
    }

    fn mark_done(&self, from: &SeatId) {
        self.done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(from.clone());
    }
}

/// What a carry did, for its caller and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Carried {
    /// A carry already recorded done: nothing moved now.
    pub already: bool,
    /// Each project whose holdings landed on `to`: its roles and the holders
    /// commit, `None` when a rerun found them carried already.
    pub landed: Vec<(String, Vec<String>, Option<String>)>,
    /// Each project whose commit was refused, with the refusal's code.
    pub refused: Vec<(String, String)>,
    /// The delivered, unanswered requests put back on their role's queue for
    /// `to`'s drain.
    pub requests: Vec<String>,
    /// Requests still `delivering` to `from` after the settle wait: the carry
    /// is not done, and the next registration of `to` or daemon start reruns.
    pub delivering: Vec<String>,
    /// The pending FYIs now held for `to`.
    pub fyis: Vec<String>,
}

/// Whether a carry may begin, or only finish one already begun.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The supersede this registration committed: record the intent, then
    /// carry.
    New,
    /// A redo: carry only a boundary whose intent is recorded.
    Redo,
}

/// How long a carry waits for a request `delivering` to the old seat to
/// settle: a send holds that state only from its claim to its mark, a few
/// milliseconds. Per `/clear`, never per tick.
const SETTLE_TRIES: u32 = 20;
const SETTLE_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// Carry `from`'s holdings and mail to `to`, once. `None` when the guard
/// spares it, or a redo finds no recorded intent.
///
/// # Errors
/// A store or spine failure; whatever was done stays done, and a rerun
/// carries the rest.
pub async fn carry(
    services: &Services,
    from: &SeatId,
    to: &SeatId,
    start: Start,
) -> pij_core::error::Result<Option<Carried>> {
    let carries = &services.carries;
    let _one = carries.lock.lock().await;
    let done = Some(Carried {
        already: true,
        ..Carried::default()
    });
    if carries.is_done(from) {
        return Ok(done);
    }
    // Done once the latest record left nothing delivering.
    if let Some(event) = services
        .spine
        .latest_matching(from, &[pij_store::CARRIED_EVENT])
        .await?
        && serde_json::from_str::<serde_json::Value>(&event.payload)
            .ok()
            .and_then(|payload| payload["delivering"].as_array().map(Vec::is_empty))
            .unwrap_or(true)
    {
        carries.mark_done(from);
        return Ok(done);
    }
    let (Some(old), Some(new)) = (
        services.registry.get(from).await?,
        services.registry.get(to).await?,
    ) else {
        return Ok(None);
    };
    if !eligible(&old, &new) {
        return Ok(None);
    }
    // The intent decides: only a carry begun here is ever redone.
    if services
        .spine
        .latest_matching(from, &[pij_store::CARRY_STARTED_EVENT])
        .await?
        .is_none()
    {
        if start == Start::Redo {
            return Ok(None);
        }
        record_intent(services, from, to).await?;
    }
    let store = services.directory.store().clone();
    let mut projects: Vec<String> = store
        .open_assignments(from.as_str())
        .await?
        .into_iter()
        .map(|row| row.project)
        .collect();
    projects.dedup();
    let repos: BTreeMap<String, PathBuf> = if projects.is_empty() {
        BTreeMap::new()
    } else {
        services
            .governance
            .store()
            .list_projects()
            .await?
            .into_iter()
            .filter_map(|project| Some((project.slug, project.repo?.into())))
            .collect()
    };
    let mut carried = Carried::default();
    for project in projects {
        let outcome = match repos.get(&project) {
            None => Err(crate::community::Refusal::new(
                "E-RS-NOT-FOUND",
                format!("project {project:?} records no repo"),
            )),
            Some(repo) => {
                services
                    .directory
                    .carry_project(&project, repo.clone(), from, &new)
                    .await
            }
        };
        match outcome {
            Ok((roles, commit)) => carried.landed.push((project, roles, commit)),
            Err(refusal) => {
                eprintln!(
                    "pij-rs carry {from} -> {to}: project {project} kept its holdings: {} {}",
                    refusal.code, refusal.message
                );
                carried.refused.push((project, refusal.code.to_string()));
            }
        }
    }
    // The requests follow the holdings that landed: every (project, role) `to`
    // holds as carried from `from`, which a rerun after a crash still finds.
    let why = format!("carried from {from} at a native session boundary (same process)");
    let mut scope: Vec<(String, String)> = store
        .open_assignments(to.as_str())
        .await?
        .into_iter()
        .filter(|row| row.why == why)
        .map(|row| (row.project, row.role))
        .collect();
    scope.sort();
    scope.dedup();
    let holdings = json!({
        "landed": carried.landed.iter().map(|(project, roles, commit)| json!({
            "project": project, "roles": roles, "commit": commit,
        })).collect::<Vec<_>>(),
        "refused": carried.refused.iter().map(|(project, code)| json!({
            "project": project, "code": code,
        })).collect::<Vec<_>>(),
    });
    // Each scoped role's admission gate, in order, around the mail
    // transaction (never around a governance commit): a drain holds it from
    // its claim until the request is delivered or back on the queue, so none
    // is mid-delivery to `from` here. A direct send releases it once its
    // claim is recorded, so a request may still be `delivering` for a few
    // milliseconds: wait for it, bounded, so it is carried with the rest.
    let mut tries = 0;
    let _gates = loop {
        let mut gates = Vec::with_capacity(scope.len());
        for (project, role) in &scope {
            gates.push(services.directory.admission(project, role).await);
        }
        tries += 1;
        if tries >= SETTLE_TRIES || store.delivering_held_by(from, &scope).await?.is_empty() {
            break gates;
        }
        drop(gates);
        tokio::time::sleep(SETTLE_PAUSE).await;
    };
    let at = now_ms();
    let (from_id, to_id) = (from.clone(), to.clone());
    let mail = services
        .event_bus
        .publish_committed_batch(async move {
            let mail = store
                .carry_mail_committed(&from_id, &to_id, &scope, holdings, at)
                .await?;
            Ok((mail.events.clone(), mail))
        })
        .await?;
    carried.requests = mail.requests;
    carried.delivering = mail.delivering;
    carried.fyis = mail.fyis;
    if carried.delivering.is_empty() {
        carries.mark_done(from);
    }
    Ok(Some(carried))
}

/// Record a carry's intent (`seat.carry.started` on `from`), before its first
/// write: the one fact a redo finishes on.
///
/// # Errors
/// A spine failure.
pub async fn record_intent(
    services: &Services,
    from: &SeatId,
    to: &SeatId,
) -> pij_core::error::Result<()> {
    services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: pij_core::wire::EVENT_VERSION,
            at: now_ms(),
            kind: pij_store::CARRY_STARTED_EVENT.to_string(),
            seat: Some(from.clone()),
            payload: json!({"from": from, "to": to}).to_string(),
        })
        .await
        .map(|_| ())
}

/// Wall-clock epoch ms.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Carry on its own task, then drain the successor's roles' queues (F193's
/// drain), so the requests the carry put back, and any queued for a carried
/// role, reach it. A carry already done drains too: a crash between its
/// commit and its drain leaves the requests queued, and this is their redo.
pub fn carry_later(
    services: &Services,
    from: &SeatId,
    to: &SeatId,
    start: Start,
) -> tokio::task::JoinHandle<()> {
    let (services, from, to) = (services.clone(), from.clone(), to.clone());
    tokio::spawn(async move {
        match carry(&services, &from, &to, start).await {
            Ok(Some(carried)) => {
                if !carried.delivering.is_empty() {
                    eprintln!(
                        "pij-rs carry {from} -> {to}: still delivering to {from}: {}; the next registration or daemon start finishes the carry",
                        carried.delivering.join(", ")
                    );
                }
                let _ = crate::delivery::role_send::drain_returned(&services, &to).await;
            }
            Ok(None) => {}
            Err(error) => eprintln!("pij-rs carry {from} -> {to}: {error}"),
        }
    })
}

/// The registration hook: carry each named (from, to) pair later.
pub(crate) fn carry_hook(services: &Services) -> crate::registration::CarryHook {
    let services = services.clone();
    std::sync::Arc::new(move |from: &SeatId, to: &SeatId, start: Start| {
        carry_later(&services, from, to, start);
    })
}

/// Once at daemon start, on its own task: finish every carry this code began
/// and did not record done (a crash between its writes), one after another,
/// each then draining the successor. A boundary with no recorded intent (one
/// from before the carry existed) is left alone. One roster read; one or two
/// spine reads per tombstone with a launch id and a live successor.
pub async fn redo_at_boot(services: &Services) {
    let seats = match services.registry.list(Default::default()).await {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs carry at boot: {error}");
            return;
        }
    };
    let live: HashSet<&SeatId> = seats
        .iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .map(|seat| &seat.id)
        .collect();
    for old in &seats {
        let Some(to) = old.tombstone_reason.as_deref().and_then(boundary_successor) else {
            continue;
        };
        if old.spawn_id.is_none() || !live.contains(&to) {
            continue;
        }
        let _ = carry_later(services, &old.id, &to, Start::Redo).await;
    }
}
