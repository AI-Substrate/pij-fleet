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
//! registration that wrote it reports, and goes to the live end of the old
//! seat's chain of native session boundaries: each tombstone names the next
//! seat as its successor in the same process, every seat names the same
//! launch (`spawn_id`, present), and the last one is live. Two `/clear`s
//! queued before the first carry runs land everything on the latest seat. A
//! carry first records its intent (`seat.carry.started` on `from`, with the
//! holdings it carries). Every redo, at a later registration or at daemon
//! start, finishes only a carry whose intent is recorded and not yet
//! completed (`seat.carried` names the intent it completes): the intent
//! decides, so a boundary from before this code existed is never carried,
//! and a seat id that comes back and crosses a later boundary is carried
//! again. BRAKES, each of which can only spare: no `spawn_id`, a different
//! one anywhere on the chain, no live end, no recorded intent on a redo, or
//! an intent already completed. The store's mail transaction proves every
//! hop again in its own read of the seats table and refuses otherwise.
//!
//! The two stores cannot share a commit. Governance first: per project, one
//! holders commit ends `from`'s holdings and opens the same for the live end,
//! and the index follows in ONE SQLite transaction
//! ([`crate::community::directory::DirectoryService::carry_project`]),
//! rebuilt from the holders files, so no cut leaves the old holdings ended
//! without the new. Then one SQLite transaction puts the requests of the
//! intent's roles back on the queue, moves the FYIs and records
//! `seat.carried` ([`pij_store::SqliteOrchestration::carry_mail_committed`]);
//! then the live end's drain. A crash between any two heals: the scope is
//! the intent's, never what the index shows on a rerun, and every step
//! checks state, so a rerun neither duplicates a holding nor moves mail
//! twice; the successor's next registration and the next daemon start redo
//! any carry not recorded done, and drain again. A project whose commit is
//! refused keeps its holdings and its requests on the old seat (recorded in
//! the event); only the seat-addressed FYIs move.
//!
//! Cost: per `/clear` only (a human action): one spine event, one holders
//! commit and one index transaction per project the old seat held roles in,
//! one SQLite transaction, then one drain; a request still `delivering` is
//! waited for at most 20 × 250 ms. Nothing per tick. At daemon start, one
//! roster read and one spine read per boundary tombstone with a launch id,
//! once; an unfinished intent adds a registry read per hop.

use std::collections::BTreeMap;
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

/// The most native session boundaries a chain is followed across (each a
/// `/clear`); a longer chain carries nothing.
const MAX_HOPS: usize = 16;

/// The guard (the policy, with its brakes), as the chain from `old` to the
/// live end of its native session boundaries: each seat's tombstone names
/// the next as its native-session-boundary successor in the same process,
/// every seat names `old`'s launch (`spawn_id`, present), and the last one
/// is live. `None` when any hop fails. A successor that was itself retired
/// by a later `/clear` before this carry ran is passed through, so two
/// `/clear`s queued before the first carry still land everything on the
/// live end (review of #279, R279-002).
///
/// # Errors
/// A registry failure.
async fn live_chain(
    services: &Services,
    old: &SeatDescriptor,
) -> pij_core::error::Result<Option<Vec<SeatDescriptor>>> {
    let Some(launch) = old.spawn_id.as_deref() else {
        return Ok(None);
    };
    let mut chain: Vec<SeatDescriptor> = Vec::new();
    let mut reason = old.tombstoned_at.and(old.tombstone_reason.clone());
    for _ in 0..MAX_HOPS {
        let Some(next) = reason.as_deref().and_then(boundary_successor) else {
            return Ok(None);
        };
        let Some(seat) = services.registry.get(&next).await? else {
            return Ok(None);
        };
        if seat.spawn_id.as_deref() != Some(launch) {
            return Ok(None);
        }
        let live = seat.tombstoned_at.is_none();
        reason = seat.tombstone_reason.clone();
        chain.push(seat);
        if live {
            return Ok(Some(chain));
        }
    }
    Ok(None)
}

/// One carry at a time.
#[derive(Default)]
pub struct Carries {
    lock: tokio::sync::Mutex<()>,
}

/// What a carry did, for its caller and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Carried {
    /// The carry's intent was already recorded done: nothing moved now.
    pub already: bool,
    /// The live end of the chain the work went to, whose roles to drain.
    pub to: Option<SeatId>,
    /// Each project whose holdings landed on `to`: its roles and the holders
    /// commit, `None` when a rerun found them carried already.
    pub landed: Vec<(String, Vec<String>, Option<String>)>,
    /// Each project whose commit was refused, with the refusal's code.
    pub refused: Vec<(String, String)>,
    /// The delivered, unanswered requests put back on their role's queue.
    pub requests: Vec<String>,
    /// Requests still `delivering` to `from` after the settle wait: the carry
    /// is not done, and the next registration or daemon start reruns.
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

/// A recorded carry intent (`seat.carry.started` on the old seat): its
/// spine seq, which `seat.carried` names when it completes, and the
/// holdings `(project, role, instance)` the old seat had when it began. The
/// scope comes from the intent, never from what the index shows on a
/// rerun, so a partial write cannot hide a project (R279-001).
#[derive(Debug)]
struct Intent {
    seq: u64,
    scope: Vec<(String, String, String)>,
}

/// The latest intent on `from`, and whether a `seat.carried` naming it
/// recorded the carry done (nothing left delivering). Completion is keyed by
/// the intent, so a seat id that comes back and crosses another boundary
/// later is carried again.
async fn latest_intent(
    services: &Services,
    from: &SeatId,
) -> pij_core::error::Result<Option<(Intent, bool)>> {
    let Some(started) = services
        .spine
        .latest_matching(from, &[pij_store::CARRY_STARTED_EVENT])
        .await?
    else {
        return Ok(None);
    };
    let Some(seq) = started.seq.map(|seq| seq.0) else {
        return Ok(None);
    };
    let payload: serde_json::Value = serde_json::from_str(&started.payload).unwrap_or_default();
    let scope = payload["scope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|held| {
            Some((
                held["project"].as_str()?.to_string(),
                held["role"].as_str()?.to_string(),
                held["instance"].as_str()?.to_string(),
            ))
        })
        .collect();
    let done = services
        .spine
        .latest_matching(from, &[pij_store::CARRIED_EVENT])
        .await?
        .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
        .is_some_and(|carried| {
            carried["intent"].as_u64() == Some(seq)
                && carried["delivering"].as_array().is_some_and(Vec::is_empty)
        });
    Ok(Some((Intent { seq, scope }, done)))
}

/// How long a carry waits for a request `delivering` to the old seat to
/// settle: a send holds that state only from its claim to its mark, a few
/// milliseconds. Per `/clear`, never per tick.
const SETTLE_TRIES: u32 = 20;
const SETTLE_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// Carry `from`'s holdings and mail to the live end of its boundary chain,
/// whose first successor is `to`, once per intent. `None` when the guard
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
    let _one = services.carries.lock.lock().await;
    // The intent decides: only a carry begun here is ever redone.
    let recorded = latest_intent(services, from).await?;
    if start == Start::Redo && recorded.is_none() {
        return Ok(None);
    }
    let chain = match services.registry.get(from).await? {
        Some(old) => live_chain(services, &old).await?,
        None => None,
    };
    let target = chain.as_ref().and_then(|chain| chain.last().cloned());
    if start == Start::Redo && recorded.as_ref().is_some_and(|(_, done)| *done) {
        return Ok(Some(Carried {
            already: true,
            to: target.map(|seat| seat.id),
            ..Carried::default()
        }));
    }
    let (Some(chain), Some(target)) = (chain, target) else {
        return Ok(None);
    };
    if chain.first().map(|seat| &seat.id) != Some(to) {
        return Ok(None);
    }
    let store = services.directory.store().clone();
    let intent = match recorded {
        Some((intent, false)) => intent,
        // A new boundary, or the same id crossing another one later.
        Some((_, true)) | None => begin(services, from, &target.id).await?,
    };
    let mut projects: Vec<&str> = intent
        .scope
        .iter()
        .map(|(project, _, _)| project.as_str())
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
    let mut carried = Carried {
        to: Some(target.id.clone()),
        ..Carried::default()
    };
    for project in projects {
        let held: Vec<(String, String)> = intent
            .scope
            .iter()
            .filter(|(scoped, _, _)| scoped == project)
            .map(|(_, role, instance)| (role.clone(), instance.clone()))
            .collect();
        let outcome = match repos.get(project) {
            None => Err(crate::community::Refusal::new(
                "E-RS-NOT-FOUND",
                format!("project {project:?} records no repo"),
            )),
            Some(repo) => {
                services
                    .directory
                    .carry_project(project, repo.clone(), from, &target, &held)
                    .await
            }
        };
        match outcome {
            Ok((roles, commit)) => carried.landed.push((project.to_string(), roles, commit)),
            Err(refusal) => {
                eprintln!(
                    "pij-rs carry {from} -> {}: project {project} kept its holdings: {} {}",
                    target.id, refusal.code, refusal.message
                );
                carried
                    .refused
                    .push((project.to_string(), refusal.code.to_string()));
            }
        }
    }
    // The requests follow the roles the intent recorded, except a refused
    // project's, which stay with their holdings.
    let mut scope: Vec<(String, String)> = intent
        .scope
        .iter()
        .filter(|(project, _, _)| {
            !carried
                .refused
                .iter()
                .any(|(refused, _)| refused == project)
        })
        .map(|(project, role, _)| (project.clone(), role.clone()))
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
    let from_id = from.clone();
    let chain_ids: Vec<SeatId> = chain.into_iter().map(|seat| seat.id).collect();
    let mail = services
        .event_bus
        .publish_committed_batch(async move {
            let mail = store
                .carry_mail_committed(&from_id, &chain_ids, &scope, holdings, intent.seq, at)
                .await?;
            Ok((mail.events.clone(), mail))
        })
        .await?;
    carried.requests = mail.requests;
    carried.delivering = mail.delivering;
    carried.fyis = mail.fyis;
    Ok(Some(carried))
}

/// Record a new carry's intent: `from`'s open holdings now, before any other
/// write. What a new carry does first.
///
/// # Errors
/// A store or spine failure.
pub async fn record_intent(
    services: &Services,
    from: &SeatId,
    to: &SeatId,
) -> pij_core::error::Result<()> {
    begin(services, from, to).await.map(|_| ())
}

/// [`record_intent`], answering the intent.
async fn begin(services: &Services, from: &SeatId, to: &SeatId) -> pij_core::error::Result<Intent> {
    let scope = services
        .directory
        .store()
        .open_assignments(from.as_str())
        .await?
        .into_iter()
        .map(|row| (row.project, row.role, row.instance))
        .collect();
    intend(services, from, to, scope).await
}

/// Append `seat.carry.started {from, to, scope}` on `from`: the one fact a
/// redo finishes on, with the holdings it carries.
///
/// # Errors
/// A spine failure.
async fn intend(
    services: &Services,
    from: &SeatId,
    to: &SeatId,
    scope: Vec<(String, String, String)>,
) -> pij_core::error::Result<Intent> {
    let held: Vec<serde_json::Value> = scope
        .iter()
        .map(|(project, role, instance)| json!({"project": project, "role": role, "instance": instance}))
        .collect();
    let seq = services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: pij_core::wire::EVENT_VERSION,
            at: now_ms(),
            kind: pij_store::CARRY_STARTED_EVENT.to_string(),
            seat: Some(from.clone()),
            payload: json!({"from": from, "to": to, "scope": held}).to_string(),
        })
        .await?;
    Ok(Intent { seq: seq.0, scope })
}

/// Wall-clock epoch ms.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Carry on its own task, then drain the live end's roles' queues (F193's
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
                if let Some(end) = carried.to {
                    let _ = crate::delivery::role_send::drain_returned(&services, &end).await;
                }
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
/// each then draining the live end of its chain. A boundary with no recorded
/// intent (one from before the carry existed) is left alone after one spine
/// read. One roster read; per tombstone with a launch id, one or two spine
/// reads and, for an intent, a registry read per hop.
pub async fn redo_at_boot(services: &Services) {
    let seats = match services.registry.list(Default::default()).await {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs carry at boot: {error}");
            return;
        }
    };
    for old in &seats {
        let Some(to) = old.tombstone_reason.as_deref().and_then(boundary_successor) else {
            continue;
        };
        if old.spawn_id.is_none() {
            continue;
        }
        let _ = carry_later(services, &old.id, &to, Start::Redo).await;
    }
}
