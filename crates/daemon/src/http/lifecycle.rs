//! Close is an owner-only tombstone. Reap is conservative stale-record repair.
//! Both native argv and typed HTTP callers enter this single semantic parser.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::{PijError, Result};
use pij_core::model::{Envelope, ErrorKind, Liveness, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{SeatFilter, Spine};
use serde::Serialize;
use serde_json::{Value, json};

use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, internal, memory_turn};
use crate::reaper;

#[derive(Clone, Copy)]
enum Operation {
    Close,
    Reap,
}
impl Operation {
    fn verb(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Reap => "reap",
        }
    }
    fn command(self) -> &'static str {
        match self {
            Self::Close => "pij close",
            Self::Reap => "pij reap",
        }
    }
}

struct Call {
    caller: CallerContext,
    attribution: Vec<SeatId>,
    seat: Option<SeatId>,
    reason: String,
    dry_run: bool,
}

fn nonempty(value: &Value, name: &str) -> std::result::Result<String, String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} must be a nonempty string"))
}

fn parse(body: Value, operation: Operation) -> std::result::Result<Call, String> {
    let object = body
        .as_object()
        .ok_or("lifecycle request must be an object")?;
    for key in object.keys() {
        let allowed = matches!(key.as_str(), "argv" | "caller" | "actor")
            || match operation {
                Operation::Close => matches!(key.as_str(), "seat" | "reason"),
                Operation::Reap => key == "dry_run",
            };
        if !allowed {
            return Err(format!("unknown {} request field: {key}", operation.verb()));
        }
    }
    let caller = object
        .get("caller")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut call = Call {
        caller,
        attribution: Vec::new(),
        seat: None,
        reason: pij_core::model::OWNER_CLOSE.to_string(),
        dry_run: false,
    };
    if let Some(actor) = object.get("actor") {
        call.attribution.push(nonempty(actor, "actor")?.into());
    }
    if let Some(argv) = object.get("argv") {
        if ["seat", "reason", "dry_run"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return Err("use typed fields or argv, not both".to_string());
        }
        let argv: Vec<String> =
            serde_json::from_value(argv.clone()).map_err(|error| error.to_string())?;
        if argv.first().map(String::as_str) != Some(operation.verb()) {
            return Err(format!("argv must begin with {}", operation.verb()));
        }
        let mut args = argv.iter().skip(1);
        let mut reason_seen = false;
        let mut actor_seen = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--json" => {}
                "--actor" if !actor_seen => {
                    actor_seen = true;
                    let actor = args
                        .next()
                        .filter(|text| !text.trim().is_empty() && !text.starts_with('-'))
                        .ok_or("--actor requires a seat")?;
                    call.attribution.push(actor.as_str().into());
                }
                "--reason" if matches!(operation, Operation::Close) && !reason_seen => {
                    reason_seen = true;
                    call.reason = args
                        .next()
                        .filter(|text| !text.trim().is_empty() && !text.starts_with('-'))
                        .ok_or("--reason requires a reason")?
                        .clone();
                }
                "--dry-run" if matches!(operation, Operation::Reap) && !call.dry_run => {
                    call.dry_run = true
                }
                value
                    if matches!(operation, Operation::Close)
                        && !value.starts_with('-')
                        && !value.trim().is_empty()
                        && call.seat.is_none() =>
                {
                    call.seat = Some(value.into())
                }
                _ => {
                    return Err(format!(
                        "unknown or repeated {} argument: {arg}",
                        operation.verb()
                    ));
                }
            }
        }
    } else {
        match operation {
            Operation::Close => {
                call.seat = object
                    .get("seat")
                    .map(|value| nonempty(value, "seat").map(SeatId::from))
                    .transpose()?;
                if let Some(reason) = object.get("reason") {
                    call.reason = nonempty(reason, "reason")?;
                }
            }
            Operation::Reap => {
                if let Some(value) = object.get("dry_run") {
                    call.dry_run = value.as_bool().ok_or("dry_run must be boolean")?;
                }
            }
        }
    }
    if matches!(operation, Operation::Close) && call.seat.is_none() {
        return Err("close requires a seat".to_string());
    }
    Ok(call)
}

fn argument_failure(command: &str, reason: String) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(command, ErrorKind::Refused, reason);
    answer.details = Some(json!({"code":"E-RS-ARG"}));
    envelope(StatusCode::BAD_REQUEST, &answer)
}

fn ownership_failure(
    command: &str,
    caller: &SeatId,
    seat: &SeatId,
    parent: Option<&SeatId>,
) -> Response {
    let operation = command.strip_prefix("pij ").unwrap_or(command);
    let mut answer: Envelope<Value> = Envelope::refused(
        command,
        ErrorKind::Refused,
        format!("E-RS-OWNERSHIP {operation}: {caller} is neither {seat} nor its recorded parent"),
    );
    answer.details = Some(
        json!({"code":"E-RS-OWNERSHIP","operation":operation,"caller":caller,"seat":seat,"parent":parent}),
    );
    envelope(StatusCode::FORBIDDEN, &answer)
}

pub(super) fn runtime_failure(command: &str, error: PijError) -> Response {
    match &error {
        PijError::GovernanceRefused { code, record } => {
            let mut answer: Envelope<Value> =
                Envelope::refused(command, ErrorKind::Refused, error.to_string());
            answer.details = Some(json!({"code":code,"record":record}));
            envelope(StatusCode::CONFLICT, &answer)
        }
        PijError::NoRegistryEntry { .. } => envelope(
            StatusCode::NOT_FOUND,
            &Envelope::<Value>::refused(command, ErrorKind::Refused, error.to_string()),
        ),
        _ => internal(command, error),
    }
}

/// What a closed seat's recorded pane holds now (F175). Pane ids are reused
/// once tmux restarts, so a kill command naming the id can kill another seat:
/// only a pane still running this seat's own process gets one.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PaneNow {
    /// The seat's process is alive with its recorded start, and the pane's root
    /// process started no later than it: its own pane (the reaper's rule for a
    /// reused pane id, plan 156 rule 2).
    Own,
    /// tmux has no such pane.
    Gone,
    /// The pane runs something that isn't this seat: the live seat recorded on
    /// it, when there is one.
    Other(Option<SeatId>),
    /// It could not be checked: no recorded process, or a read failed.
    Unchecked,
}

/// The seat's recorded process, as liveness observed it.
#[derive(Clone, Copy, Debug)]
pub(super) enum SeatProcess {
    /// Alive with this recorded start.
    Alive(u64),
    /// Dead, or its pid now belongs to a later process.
    Gone,
    /// Never recorded, or the read failed.
    Unknown,
}

/// Decide [`PaneNow`] from what tmux and liveness show. `root_start` is the
/// pane's root process start (`None` when unreadable); `holder` is another
/// live seat recorded on the same pane, which always vetoes `Own`: a native
/// successor keeps its predecessor's process and pane (review of #220,
/// S220-001), and withholding the command is only ever more conservative.
pub(super) fn judge_pane(
    root_start: Option<u64>,
    process: SeatProcess,
    holder: Option<SeatId>,
) -> PaneNow {
    if holder.is_some() {
        return PaneNow::Other(holder);
    }
    match (process, root_start) {
        (SeatProcess::Alive(start), Some(root)) if root <= start => PaneNow::Own,
        (SeatProcess::Alive(_), Some(_)) | (SeatProcess::Gone, _) => PaneNow::Other(None),
        (SeatProcess::Alive(_), None) | (SeatProcess::Unknown, _) => PaneNow::Unchecked,
    }
}

/// Read what `seat`'s recorded pane holds now: one tmux read, only over an
/// already-live control client (never a start or a fork), then in-process
/// liveness and the registry.
pub(super) async fn pane_now(state: &AppState, seat: &SeatDescriptor) -> PaneNow {
    let Some(pane) = seat.pane.as_deref() else {
        return PaneNow::Unchecked;
    };
    let liveness = state.services.liveness.as_ref();
    let root_start = match state.services.tmux.pane_process_without_launch(pane).await {
        Ok(Some(Some(root))) => liveness.proc_start(root.pid).await.ok().flatten(),
        Ok(Some(None)) => return PaneNow::Gone,
        Ok(None) | Err(_) => return PaneNow::Unchecked,
    };
    let process = match seat.proc {
        Some(proc) => match pij_core::liveness::alive(proc, liveness).await {
            Ok(Liveness::Active) => SeatProcess::Alive(proc.proc_start),
            Ok(Liveness::Dead { .. } | Liveness::Recycled { .. }) => SeatProcess::Gone,
            Err(_) => SeatProcess::Unknown,
        },
        None => SeatProcess::Unknown,
    };
    let holder = state
        .services
        .registry
        .list(SeatFilter::default())
        .await
        .ok()
        .and_then(|seats| {
            seats
                .into_iter()
                .find(|other| {
                    other.id != seat.id
                        && other.tombstoned_at.is_none()
                        && other.pane.as_deref() == Some(pane)
                })
                .map(|other| other.id)
        });
    judge_pane(root_start, process, holder)
}

/// What a close leaves running (F149): it retires the seat, never its process
/// or pane. The seat can no longer register under its id; the process stays
/// until its parent kills the pane, which is named only while it is still
/// this seat's own (F175).
pub(super) fn process_note(pane: Option<&str>, now: &PaneNow) -> String {
    let pane = pane.filter(|pane| {
        pane.len() > 1
            && pane.starts_with('%')
            && pane[1..].bytes().all(|byte| byte.is_ascii_digit())
    });
    let Some(pane) = pane else {
        return "pij close retires the seat; its process keeps running. To stop it, its parent kills its pane (tmux kill-pane -t <pane>)".to_string();
    };
    match now {
        PaneNow::Own => format!(
            "pij close retires the seat; its process keeps running. To stop it, its parent runs: tmux kill-pane -t {pane}"
        ),
        PaneNow::Gone => {
            format!("pij close retires the seat. Its pane {pane} is gone: nothing to kill.")
        }
        PaneNow::Other(Some(seat)) => format!(
            "pij close retires the seat. Pane {pane} now runs {}, not this seat: nothing to kill.",
            pij_core::render::seat(seat.as_str())
        ),
        PaneNow::Other(None) => format!(
            "pij close retires the seat. Pane {pane} now runs another process, not this seat's: nothing to kill."
        ),
        PaneNow::Unchecked => format!(
            "pij close retires the seat; its process, if any, keeps running. Pane {pane} could not be checked as still this seat's, so pij names no kill command: look with `tmux list-panes -a` first."
        ),
    }
}

/// Who may close `seat` (one predicate for the close route and the close held
/// for a memory turn, review of #220 F001): the seat itself, its recorded
/// parent, or a designated prime for a dead parentless seat.
pub(super) async fn may_close(state: &AppState, actor: &SeatId, seat: &SeatDescriptor) -> bool {
    actor == &seat.id
        || seat.parent.as_ref() == Some(actor)
        || prime_closes_dead_orphan(state, actor, seat).await
}

/// F175: a parentless seat whose process is observed dead has no owner left
/// to close it, so a project's designated prime (C24) may. A live or
/// unobservable process, a recorded parent, or a caller no project designates
/// keeps the ownership refusal.
async fn prime_closes_dead_orphan(state: &AppState, actor: &SeatId, seat: &SeatDescriptor) -> bool {
    if seat.parent.is_some() {
        return false;
    }
    let Some(proc) = seat.proc else {
        return false;
    };
    if !matches!(
        pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await,
        Ok(Liveness::Dead { .. } | Liveness::Recycled { .. })
    ) {
        return false;
    }
    state
        .services
        .governance
        .store()
        .list_projects()
        .await
        .is_ok_and(|projects| {
            projects
                .iter()
                .any(|project| project.prime_id.as_ref() == Some(actor))
        })
}

#[derive(Serialize)]
pub(super) struct CloseReceipt {
    seat: SeatId,
    tombstoned_at: u64,
    reason: String,
    pub(super) seq: Seq,
}

/// End the closed seat's role holdings (plan 195, AC4). Never fails the close:
/// a refused write leaves the ends pending, named in the receipt, for the
/// project's next directory write. A seat with no holdings costs one query.
async fn end_holdings(
    state: &AppState,
    seat: &SeatId,
    actor: &SeatId,
) -> crate::community::directory::CloseEnds {
    let repos: std::collections::BTreeMap<String, std::path::PathBuf> = state
        .services
        .governance
        .store()
        .list_projects()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|project| Some((project.slug, project.repo?.into())))
        .collect();
    state
        .services
        .directory
        .end_for_close(seat, actor, |project| repos.get(project).cloned())
        .await
}

pub(super) async fn tombstone_receipt(
    spine: &dyn Spine,
    seat: &SeatId,
    expected_seq: Option<Seq>,
    existing: Option<&SeatDescriptor>,
) -> Result<Option<CloseReceipt>> {
    let inconsistent = || PijError::Adapter {
        adapter: "lifecycle/tombstone-receipt".to_string(),
        message: format!("no matching durable tombstone event for {seat}"),
    };
    let Some(event) = spine.latest_matching(seat, &["seat.tombstone"]).await? else {
        return Ok(None);
    };
    let seq = event.seq.ok_or_else(inconsistent)?;
    let payload: Value = serde_json::from_str(&event.payload).map_err(|_| inconsistent())?;
    let reason = payload["reason"].as_str().ok_or_else(inconsistent)?;
    if expected_seq.is_some_and(|expected| expected != seq) {
        return Err(inconsistent());
    }
    if existing.is_some_and(|raw| {
        raw.tombstoned_at != Some(event.at) || raw.tombstone_reason.as_deref() != Some(reason)
    }) {
        return Ok(None);
    }
    Ok(Some(CloseReceipt {
        seat: seat.clone(),
        tombstoned_at: event.at,
        reason: reason.to_string(),
        seq,
    }))
}

pub(crate) async fn close(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    handle(state, body, Operation::Close).await
}

pub(crate) async fn reap(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    handle(state, body, Operation::Reap).await
}

async fn handle(
    state: AppState,
    body: std::result::Result<Json<Value>, JsonRejection>,
    operation: Operation,
) -> Response {
    let command = operation.command();
    let call = match body
        .map_err(|error| error.body_text())
        .and_then(|Json(body)| parse(body, operation))
    {
        Ok(call) => call,
        Err(reason) => return argument_failure(command, reason),
    };
    let actor = match resolve_seat(&state, command, call.caller.session_id, call.caller.pane).await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    if call.attribution.iter().any(|claimed| claimed != &actor.id) {
        return ownership_failure(
            command,
            &actor.id,
            call.seat.as_ref().unwrap_or(&actor.id),
            actor.parent.as_ref(),
        );
    }
    match operation {
        Operation::Reap => match reaper::reap(
            state.services.registry.as_ref(),
            state.services.liveness.as_ref(),
            state.services.tmux.as_ref(),
            call.dry_run,
        )
        .await
        {
            Ok(receipt) => envelope(StatusCode::OK, &Envelope::ok(command, receipt)),
            Err(error) => runtime_failure(command, error),
        },
        Operation::Close => {
            let Some(target) = call.seat else {
                return argument_failure(command, "close requires a seat".to_string());
            };
            let expected = match state.services.registry.get(&target).await {
                Ok(Some(expected)) => expected,
                Ok(None) => {
                    return runtime_failure(
                        command,
                        PijError::NoRegistryEntry {
                            seat: target,
                            store: "the daemon's configured registry".to_string(),
                        },
                    );
                }
                Err(error) => return runtime_failure(command, error),
            };
            if !may_close(&state, &actor.id, &expected).await {
                return ownership_failure(command, &actor.id, &target, expected.parent.as_ref());
            }
            // Plan 201: a role holder writes its memory before it is closed,
            // and so does a seat that released one in this context (F130).
            let turn = if expected.tombstoned_at.is_none() {
                memory_turn::turn_for(&state, &target).await
            } else {
                None
            };
            let Some(turn) = turn else {
                return match close_seat(
                    &state,
                    command,
                    &actor.id,
                    target,
                    call.reason,
                    Some(expected),
                )
                .await
                {
                    Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
                    Err(response) => *response,
                };
            };
            let note = process_note(expected.pane.as_deref(), &pane_now(&state, &expected).await);
            match memory_turn::hold(
                &state,
                command,
                expected,
                turn,
                memory_turn::Then::Close {
                    actor: actor.id,
                    reason: call.reason,
                },
            ) {
                Ok(held) => {
                    let mut data = serde_json::to_value(&held).unwrap_or(Value::Null);
                    data["process"] = note.into();
                    envelope(StatusCode::OK, &Envelope::ok(command, data))
                }
                Err(refusal) => *refusal,
            }
        }
    }
}

/// Tombstone `target` and end its holdings, returning the receipt data or the
/// refusal. `expected` is the row the caller checked ownership against; `None`
/// reads it again (after a memory turn).
pub(super) async fn close_seat(
    state: &AppState,
    command: &str,
    actor: &SeatId,
    target: SeatId,
    reason: String,
    expected: Option<SeatDescriptor>,
) -> std::result::Result<Value, Box<Response>> {
    // Recorded as an owner's close, so the seat's own next registration can't
    // lift it (F149).
    let reason = pij_core::model::owner_close_reason(&reason);
    let expected = match expected {
        Some(expected) => expected,
        None => match state.services.registry.get(&target).await {
            Ok(Some(expected)) => expected,
            Ok(None) => {
                return Err(Box::new(runtime_failure(
                    command,
                    PijError::NoRegistryEntry {
                        seat: target,
                        store: "the daemon's configured registry".to_string(),
                    },
                )));
            }
            Err(error) => return Err(Box::new(runtime_failure(command, error))),
        },
    };
    let note = process_note(expected.pane.as_deref(), &pane_now(state, &expected).await);
    // A repeat close returns the existing owner-close. A seat retired for any
    // other reason (observed dead, reaped) is closed again as its owner's, so
    // its own session can't lift it either (F149, review of #200).
    let owner_closed = expected.tombstoned_at.is_some()
        && expected
            .tombstone_reason
            .as_deref()
            .is_some_and(pij_core::model::is_owner_close);
    let receipt = if owner_closed {
        tombstone_receipt(
            state.services.spine.as_ref(),
            &target,
            None,
            Some(&expected),
        )
        .await
    } else {
        match state
            .services
            .registry
            .tombstone_if_unchanged(expected, reason)
            .await
        {
            Ok(seq) => {
                tombstone_receipt(state.services.spine.as_ref(), &target, Some(seq), None).await
            }
            Err(error) => Err(error),
        }
    };
    match receipt {
        Ok(Some(receipt)) => {
            let ends = end_holdings(state, &target, actor).await;
            let mut data = serde_json::to_value(&receipt).unwrap_or(Value::Null);
            data["process"] = note.into();
            if !ends.committed.is_empty() || !ends.pending.is_empty() {
                data["directory"] = serde_json::to_value(&ends).unwrap_or(Value::Null);
            }
            Ok(data)
        }
        Ok(None) => Err(Box::new(runtime_failure(
            command,
            PijError::Adapter {
                adapter: "lifecycle/tombstone-receipt".to_string(),
                message: format!("no matching durable tombstone event for {target}"),
            },
        ))),
        Err(error) => Err(Box::new(runtime_failure(command, error))),
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use pij_core::model::{Harness, PaneProcess, ProcIdentity};
    use pij_core::orchestration::Project;
    use pij_core::ports::Registry;
    use pij_testkit::fakes::{FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux};

    use super::*;
    use crate::http::router_with_config;
    use crate::http::tests::{config, spawn, test_services};

    const SEAT_START: u64 = 1_000;

    fn eel() -> SeatId {
        "pij-estimated-eel".into()
    }

    #[test]
    fn only_a_pane_still_running_the_seats_process_is_its_own() {
        let alive = SeatProcess::Alive(SEAT_START);
        // The pane's root is as old as the seat's process, or older.
        assert_eq!(judge_pane(Some(SEAT_START), alive, None), PaneNow::Own);
        assert_eq!(judge_pane(Some(SEAT_START - 5), alive, None), PaneNow::Own);
        // Review of #220 (S220-001, F002): two live records on one pane, root
        // older than both, as a native successor that kept its predecessor's
        // process and pane. The other record vetoes the command.
        for root in [SEAT_START - 5, SEAT_START] {
            assert_eq!(
                judge_pane(Some(root), alive, Some(eel())),
                PaneNow::Other(Some(eel()))
            );
        }
        // F175: tmux restarted and the id came back on a later pane.
        assert_eq!(
            judge_pane(Some(SEAT_START + 60), alive, Some(eel())),
            PaneNow::Other(Some(eel()))
        );
        assert_eq!(
            judge_pane(Some(SEAT_START + 60), alive, None),
            PaneNow::Other(None)
        );
        // A dead seat has no process left to kill in any pane.
        for holder in [Some(eel()), None] {
            assert_eq!(
                judge_pane(Some(SEAT_START - 5), SeatProcess::Gone, holder.clone()),
                PaneNow::Other(holder)
            );
        }
        // What can't be observed never earns a kill command.
        assert_eq!(judge_pane(None, alive, None), PaneNow::Unchecked);
        assert_eq!(
            judge_pane(Some(SEAT_START), SeatProcess::Unknown, None),
            PaneNow::Unchecked
        );
        assert_eq!(
            judge_pane(None, SeatProcess::Unknown, Some(eel())),
            PaneNow::Other(Some(eel()))
        );
    }

    #[test]
    fn the_kill_command_is_named_only_for_the_seats_own_pane() {
        let own = process_note(Some("%61"), &PaneNow::Own);
        assert!(
            own.ends_with("its parent runs: tmux kill-pane -t %61"),
            "{own}"
        );
        for (now, says) in [
            (PaneNow::Gone, "Its pane %61 is gone: nothing to kill."),
            (
                PaneNow::Other(Some(eel())),
                "Pane %61 now runs pij-estimated-eel, not this seat: nothing to kill.",
            ),
            (
                PaneNow::Other(None),
                "Pane %61 now runs another process, not this seat's: nothing to kill.",
            ),
            (PaneNow::Unchecked, "Pane %61 could not be checked"),
        ] {
            let note = process_note(Some("%61"), &now);
            assert!(note.contains(says), "{note}");
            assert!(!note.contains("kill-pane -t %61"), "{note}");
        }
    }

    fn seat(
        id: &str,
        parent: Option<&str>,
        pane: Option<&str>,
        proc: Option<(u32, u64)>,
    ) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Claude, "/abs/tree");
        seat.parent = parent.map(SeatId::from);
        seat.pane = pane.map(str::to_string);
        seat.proc = proc.map(|(pid, proc_start)| ProcIdentity { pid, proc_start });
        seat
    }

    /// A daemon with `seats`, tmux panes running `panes` (pane, root pid) and
    /// `alive` processes; `pij-prime` is the designated prime of one project.
    async fn world(
        seats: Vec<SeatDescriptor>,
        panes: &[(&str, u32)],
        alive: &[(u32, u64)],
    ) -> (SocketAddr, Arc<FakeRegistry>, tokio::task::JoinHandle<()>) {
        let registry = Arc::new(FakeRegistry::new());
        for seat in seats {
            registry.put(seat).await.expect("seed seat");
        }
        let mut services = test_services(
            registry.clone(),
            Arc::new(FakeQueue::new(1_024).expect("queue")),
            Arc::new(FakeSpine::new()),
        )
        .await;
        let mut tmux = FakeTmux::new();
        for (pane, pid) in panes {
            tmux = tmux.with_pane_process(
                pane,
                PaneProcess {
                    pid: *pid,
                    cwd: "/abs/tree".to_string(),
                },
            );
        }
        services.tmux = Arc::new(tmux);
        services.liveness = Arc::new(alive.iter().fold(
            FakeLiveness::new(),
            |fake, (pid, start)| {
                fake.with_proc(ProcIdentity {
                    pid: *pid,
                    proc_start: *start,
                })
            },
        ));
        assert!(
            services
                .governance
                .store()
                .create_project(&Project {
                    slug: "fleet".to_string(),
                    description: None,
                    repo: None,
                    plan_path: None,
                    prime_id: Some("pij-prime".into()),
                    created_by: "pij-prime".into(),
                    created_at: 1,
                })
                .await
                .expect("project")
        );
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        (addr, registry, server)
    }

    async fn close(addr: SocketAddr, caller: &str, target: &str) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/close"))
            .bearer_auth("key")
            .json(&json!({"seat": target, "caller": {"PIJ_SESSION_ID": caller}}))
            .send()
            .await
            .expect("close request");
        let status = response.status().as_u16();
        (status, response.json().await.expect("close envelope"))
    }

    /// F175: tmux restarted, and %61 now runs pij-estimated-eel. Closing the
    /// dead seat that used to live there must not hand its parent a command
    /// that kills eel; closing a live seat in its own pane still names it.
    #[tokio::test]
    async fn a_reused_pane_gets_no_kill_command_and_the_seats_own_pane_does() {
        let (addr, _, server) = world(
            vec![
                seat("pij-parent", None, None, None),
                seat(
                    "pij-old",
                    Some("pij-parent"),
                    Some("%61"),
                    Some((500, SEAT_START)),
                ),
                seat("pij-estimated-eel", None, Some("%61"), Some((600, 5_000))),
                seat(
                    "pij-worker",
                    Some("pij-parent"),
                    Some("%62"),
                    Some((700, 2_000)),
                ),
                // A native successor keeps its predecessor's process and pane.
                seat(
                    "pij-predecessor",
                    Some("pij-parent"),
                    Some("%65"),
                    Some((1_100, 3_000)),
                ),
                seat("pij-successor", None, Some("%65"), Some((1_100, 3_000))),
            ],
            &[("%61", 599), ("%62", 699), ("%65", 1_099)],
            &[
                (599, 4_990),
                (600, 5_000),
                (699, 1_990),
                (700, 2_000),
                (1_099, 2_990),
                (1_100, 3_000),
            ],
        )
        .await;
        let (status, old) = close(addr, "pij-parent", "pij-old").await;
        assert_eq!(status, 200, "{old}");
        let note = old["data"]["process"].as_str().unwrap_or_default();
        assert!(
            note.contains("Pane %61 now runs pij-estimated-eel, not this seat: nothing to kill.")
                && !note.contains("kill-pane"),
            "{old}"
        );
        // Review of #220 (S220-001, F002): the predecessor's process and root
        // pass, but the pane's live successor vetoes the command.
        let (status, predecessor) = close(addr, "pij-parent", "pij-predecessor").await;
        assert_eq!(status, 200, "{predecessor}");
        let note = predecessor["data"]["process"].as_str().unwrap_or_default();
        assert!(
            note.contains("Pane %65 now runs pij-successor, not this seat: nothing to kill.")
                && !note.contains("kill-pane"),
            "{predecessor}"
        );
        let (status, worker) = close(addr, "pij-parent", "pij-worker").await;
        assert_eq!(status, 200, "{worker}");
        assert!(
            worker["data"]["process"]
                .as_str()
                .is_some_and(|note| note.ends_with("its parent runs: tmux kill-pane -t %62")),
            "{worker}"
        );
        server.abort();
    }

    /// F175: a parentless seat whose process is dead has no owner left to close
    /// it; the designated prime may. Anyone else, or a live seat, still refuses.
    #[tokio::test]
    async fn the_designated_prime_closes_a_dead_parentless_seat_and_no_one_else_does() {
        let (addr, registry, server) = world(
            vec![
                seat("pij-prime", None, None, None),
                seat("pij-outsider", None, None, None),
                seat("pij-orphan", None, Some("%63"), Some((800, 3_000))),
                seat("pij-live-orphan", None, Some("%64"), Some((900, 3_000))),
            ],
            &[("%64", 899)],
            &[(899, 2_990), (900, 3_000)],
        )
        .await;
        let (status, refused) = close(addr, "pij-outsider", "pij-orphan").await;
        assert_eq!(status, 403, "{refused}");
        assert_eq!(refused["details"]["code"], "E-RS-OWNERSHIP");
        let (status, live) = close(addr, "pij-prime", "pij-live-orphan").await;
        assert_eq!(
            status, 403,
            "a live parentless seat is not the prime's to close: {live}"
        );
        let (status, closed) = close(addr, "pij-prime", "pij-orphan").await;
        assert_eq!(status, 200, "{closed}");
        assert!(
            closed["data"]["process"]
                .as_str()
                .is_some_and(|note| note.contains("Its pane %63 is gone: nothing to kill.")),
            "{closed}"
        );
        let orphan = registry.get(&"pij-orphan".into()).await.unwrap().unwrap();
        assert!(orphan.tombstoned_at.is_some());
        let live = registry
            .get(&"pij-live-orphan".into())
            .await
            .unwrap()
            .unwrap();
        assert!(live.tombstoned_at.is_none());
        server.abort();
    }
}
