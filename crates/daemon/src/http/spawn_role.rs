//! `pij spawn --as role:<id>[/<instance>]` (PRD 007 req-07, plan 242): one
//! command launches a seat with the role's launch settings, gives it the role
//! and delivers its first turn.
//!
//! [`prepare`] runs before any launch: it resolves the project from the
//! spawn's folder, reads the role, fills its parameters and assembles the
//! first turn, so every refusal there launches nothing. [`commit`] runs after
//! the seat's row and before the bind wait: ONE writer commit records the first
//! turn and the holding (the same commit one `pij directory assign` makes),
//! then the first turn is queued from its creator, then the role's queue
//! drains. The seat receives nothing before that commit.
//!
//! Cost: in-process reads before the launch; after it, the one writer commit
//! an assign costs. No loop, poll or timer.

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, SeatDescriptor, SeatId};
use serde_json::{Value, json};

use super::directory::{parameter_sets, project_and_repo, read_packet};
use super::identity::{self, CallerContext};
use super::routes::Trigger;
use super::types::{AsRole, SpawnRequest};
use super::{AppState, envelope};
use crate::community::Refusal;
use crate::community::directory::AssignRequest;
use crate::community::first_turn;

/// A role spawn read and checked before its launch.
pub(super) struct Prepared {
    creator: SeatId,
    project: String,
    repo: std::path::PathBuf,
    role: String,
    instance: String,
    first_turn: String,
    warnings: Vec<String>,
    role_type: Option<String>,
    why: String,
    pod: Option<String>,
}

/// The warning a spawn with no role carries (PRD 007 req-07): it launches as
/// before and says how to start one in a role.
pub(super) const NO_ROLE: &str = "this seat starts in no role. Start it in one with `pij spawn ... --as role:<id>`; a role that does not exist yet is added first with `pij directory add <id> --type <type> --why <text> ...` (`pij directory --help`)";

/// Read the role, fill its parameters and build its first turn, before any
/// launch. Fills the spawn's model and effort from the role's launch settings
/// when it gives none and runs the role's harness.
///
/// # Errors
/// A response naming the step that refused; nothing was launched.
pub(super) async fn prepare(
    state: &AppState,
    request: &mut SpawnRequest,
    as_role: AsRole,
    command_name: &'static str,
) -> Result<Prepared, Box<Response>> {
    if request.role.is_some() {
        return Err(Box::new(step_refused(
            command_name,
            "reading the role",
            &Refusal::new(
                "E-RS-ARG",
                "--as role:<id> gives the seat its role; drop --role",
            ),
        )));
    }
    if as_role.role == "prime" {
        return Err(Box::new(step_refused(
            command_name,
            "reading the role",
            &Refusal::new(
                crate::community::directory::E_PRIME_DESIGNATED,
                "the prime is designated, never started in a role: pij project set <slug> --prime <seat>",
            ),
        )));
    }
    let caller = request.caller.take().unwrap_or_default();
    let creator = match identity::resolve_seat(
        state,
        command_name,
        caller.session_id.clone(),
        caller.pane.clone(),
    )
    .await
    {
        identity::Resolved::Seat(seat, _) => seat.id,
        identity::Resolved::Refusal(response) => return Err(Box::new(response)),
    };
    let folder = CallerContext {
        cwd: Some(request.cwd.clone()),
        ..CallerContext::default()
    };
    let refuse =
        |step: &'static str| move |refusal: Refusal| step_refused(command_name, step, &refusal);
    let (project, repo) = project_and_repo(state, &folder, None)
        .await
        .map_err(refuse("finding the project from the spawn's folder"))?;
    let root = crate::community::root::find(&repo).map_err(refuse("finding governance"))?;
    if let Some(pod) = &as_role.pod {
        super::pod::recorded(&root.worktree, &project, pod).map_err(refuse("finding the pod"))?;
    }
    let packet = as_role
        .packet
        .as_deref()
        .map(|file| read_packet(&caller, file, &repo))
        .transpose()
        .map_err(refuse("reading --packet"))?;
    let sets = parameter_sets(&as_role.set).map_err(refuse("filling the role's parameters"))?;
    let start = first_turn::start(
        &root.worktree,
        &as_role.role,
        &as_role.instance,
        &sets,
        &first_turn::Creator {
            seat: creator.to_string(),
            focus: as_role.focus.clone(),
            packet,
            subject: as_role.subject.clone(),
            fresh: as_role.fresh,
        },
    )
    .map_err(refuse("building the first turn"))?;
    let focus_line = as_role
        .focus
        .clone()
        .unwrap_or_else(|| "no focus given".to_string());
    let seat = &start.default_seat;
    let same_harness = seat["harness"]
        .as_str()
        .is_none_or(|harness| harness == request.harness.to_string());
    if same_harness {
        if request.model.is_none() {
            request.model = seat["model"].as_str().map(str::to_string);
        }
        if request.effort.is_none() {
            request.effort = seat["effort"].as_str().map(str::to_string);
        }
    }
    Ok(Prepared {
        creator,
        project,
        repo,
        role: as_role.role,
        instance: as_role.instance,
        first_turn: start.first_turn,
        warnings: start.warnings,
        role_type: start.role_type,
        why: focus_line,
        pod: as_role.pod,
    })
}

/// After the seat's row and before the bind wait: one writer commit records
/// the first turn and the holding; then the first turn is queued from its
/// creator, so it is the seat's first message at bind; then the role's queue
/// drains and the routing table is held for the seat.
///
/// # Errors
/// The commit's refusal: the caller rolls the spawn back.
pub(super) async fn commit(
    state: &AppState,
    prepared: Prepared,
    seat: &SeatDescriptor,
) -> Result<Value, Refusal> {
    let Prepared {
        creator,
        project,
        repo,
        role,
        instance,
        first_turn,
        mut warnings,
        role_type,
        why,
        pod,
    } = prepared;
    let directory = &state.services.directory;
    let path = crate::community::layout::RolePaths::new(&role)?.first_turn(seat.id.as_str());
    let landed = directory
        .assign_landing(vec![AssignRequest {
            project: project.clone(),
            repo: repo.clone(),
            role: role.clone(),
            instance: instance.clone(),
            seat: seat.clone(),
            actor: creator.clone(),
            why: format!("started in the role by {creator} (pij spawn --as)"),
            machine: state.machine_alias.clone(),
            first_turn: Some(first_turn.clone()),
        }])
        .await;
    // Review of #345, N1: only a commit that did not land rolls the spawn
    // back. Once it landed, the holding and the first turn are on the branch,
    // so the seat stays and the indexing failure is named with its fix.
    let (commit, seat_role) = match landed {
        Ok(mut receipts) => {
            let receipt = receipts.remove(0);
            (receipt.commit, receipt.seat_role)
        }
        Err(crate::community::directory::AssignError::Refused(refusal)) => return Err(refusal),
        Err(crate::community::directory::AssignError::Landed { commit, refusal }) => {
            warnings.push(format!(
                "the role commit {commit} landed, but pij could not index it ({refusal}); the seat keeps its role on the branch. Index it: pij directory rebuild --project {project}"
            ));
            (commit, None)
        }
    };
    let queued = match state
        .services
        .delivery
        .send(creator.clone(), seat.id.clone(), first_turn)
        .await
    {
        Ok(receipt) => json!(receipt.msg_id),
        Err(error) => {
            warnings.push(format!(
                "the first turn is recorded at {} but could not be queued ({error}); send it: pij send {} --body-file <that file>",
                path.display(),
                seat.id
            ));
            Value::Null
        }
    };
    let drained = directory.drain(&project, &role, &instance, &seat.id).await;
    let routes = super::routes::deliver(
        &state.services,
        &seat.id,
        Some(&project),
        Trigger::Assign,
        None,
    )
    .await;
    // PRD 007 req-07: anyone may start a seat in a role; the prime is told.
    let address = if instance.is_empty() {
        pij_core::render::name(&role).into_owned()
    } else {
        format!(
            "{}/{}",
            pij_core::render::name(&role),
            pij_core::render::name(&instance)
        )
    };
    let prime_told = super::routes::tell_prime(
        &state.services,
        &project,
        &creator,
        format!(
            "pij: {} started {} in role {} ({}) in project {project}: {}",
            pij_core::render::seat(creator.as_str()),
            pij_core::render::seat(seat.id.as_str()),
            address,
            pij_core::render::word(role_type.as_deref().unwrap_or("none")),
            super::routes::quoted(&why)
        ),
    )
    .await;
    // PRD 008 req-04: a member started into a pod. The seat has landed, so
    // a failure here is a warning naming the fix, never a rollback.
    let pod = match pod {
        None => Value::Null,
        Some(pod) => {
            match super::pod::join(
                state, &project, &repo, &pod, &role, &instance, &seat.id, &creator,
            )
            .await
            {
                Ok(joined) => joined,
                Err(refusal) => {
                    warnings.push(format!(
                        "the seat started but pij could not record it in pod {pod} ({refusal}); see pij pod show {pod}"
                    ));
                    Value::Null
                }
            }
        }
    };
    Ok(json!({
        "pod": pod,
        "prime_told": prime_told,
        "role": {
            "project": project,
            "role": role,
            "instance": (!instance.is_empty()).then_some(instance),
            "creator": creator,
            "commit": commit,
            "first_turn": path.display().to_string(),
            "first_turn_msg_id": queued,
            "seat_role": seat_role,
            "drained": drained,
            "routes": routes,
        },
        "warnings": warnings,
    }))
}

/// A refusal of a role spawn, naming the step that refused it.
pub(super) fn step_refused(command_name: &str, step: &str, refusal: &Refusal) -> Response {
    let mut body = Envelope::<Value>::refused(
        command_name,
        ErrorKind::Refused,
        format!("spawn --as: {step}: {refusal}"),
    );
    body.details = Some(json!({"code": refusal.code, "next": refusal.next, "step": step}));
    envelope(StatusCode::CONFLICT, &body)
}
