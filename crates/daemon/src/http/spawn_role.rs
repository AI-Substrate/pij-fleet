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
//! A `--pod` join builds its first turn from the pod's record, as `pod up`
//! does ([`from_pod`]).
//!
//! Cost: in-process reads before the launch (a `--pod` join adds one read of
//! the pod's template); after it, the one writer commit an assign costs. No
//! loop, poll, timer or extra launch.

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, SeatDescriptor, SeatId};
use serde_json::{Value, json};

use super::directory::{own_session_files, parameter_sets, project_and_repo_routed, read_packet};
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
/// How `spawn --as` names a project: by the folder it runs in. spawn has no
/// `--project` flag, so its refusals never point at one (report of #345).
pub const SPAWN_PROJECT_ROUTE: &str =
    "spawn it from a folder in the project's repo: --cwd <folder>";

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
    mut as_role: AsRole,
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
        identity::Resolved::Seat(seat, _) => seat,
        identity::Resolved::Refusal(response) => return Err(Box::new(response)),
    };
    let own_files = own_session_files(&creator);
    let creator = creator.id;
    // Finding FLT-SPAWNAS-PARENT: a seat started in a role is parented to
    // the seat that started it, as `pod up` does, unless --parent names one.
    // A POLICY: it decides the parent. A caller pij cannot identify was
    // refused above, before any launch, so there is no parentless case.
    if request.parent.is_none() {
        request.parent = Some(creator.clone());
    }
    let folder = CallerContext {
        cwd: Some(request.cwd.clone()),
        ..CallerContext::default()
    };
    let refuse =
        |step: &'static str| move |refusal: Refusal| step_refused(command_name, step, &refusal);
    let (project, repo) = project_and_repo_routed(state, &folder, None, SPAWN_PROJECT_ROUTE)
        .await
        .map_err(refuse("finding the project from the spawn's folder"))?;
    let root = crate::community::root::find(&repo).map_err(refuse("finding governance"))?;
    let mut joining = Vec::new();
    if let Some(pod) = as_role.pod.clone() {
        let document = super::pod::recorded(&root.worktree, &project, &pod)
            .map_err(refuse("finding the pod"))?;
        let (_, turn) =
            super::pod::joining_turn(&repo, &document, &as_role.role, &as_role.instance);
        joining = from_pod(&mut as_role, &pod, turn);
    }
    let packet = as_role
        .packet
        .as_deref()
        .map(|file| read_packet(&caller, file, &repo, own_files.as_deref()))
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
        warnings: joining.into_iter().chain(start.warnings).collect(),
        role_type: start.role_type,
        why: focus_line,
        pod: as_role.pod,
    })
}

/// A member joining pod `pod` (`spawn --as --pod`, `pod add role:`) starts
/// with the first turn the pod planned for its slot (ACC-POD-LATEJOIN-FRESH):
/// the pod's recorded values replace the caller's for the slot's parameters,
/// its seed, the subject and fresh. A POLICY: it decides what the first turn
/// holds. The caller's other parameters and its packet stay; each value of
/// the caller's the pod overrides is named in a warning.
fn from_pod(as_role: &mut AsRole, pod: &str, turn: crate::community::pod::SlotTurn) -> Vec<String> {
    let mut warnings = Vec::new();
    let recorded = turn.pairs();
    let mut sets = turn.sets;
    for set in std::mem::take(&mut as_role.set) {
        let name = set.split_once('=').map_or(set.as_str(), |(name, _)| name);
        match recorded.iter().find(|(given, _)| given == name) {
            None => sets.push(set),
            Some((_, value)) if set == format!("{name}={value}") => {}
            Some((_, value)) => warnings.push(format!(
                "--set {set}: pod {pod} records {name}={value}, which this member uses; change it for every member with pij pod set {pod} {name}=<value>"
            )),
        }
    }
    as_role.set = sets;
    if let Some(seed) = turn.focus {
        if as_role.focus.as_ref().is_some_and(|focus| *focus != seed) {
            warnings.push(format!(
                "--focus: this member's slot in pod {pod} has its own seed, which this member gets instead; drop --focus, or change the slot's seed in the pod's template between runs"
            ));
        }
        as_role.focus = Some(seed);
    }
    if let Some(subject) = turn.subject {
        if as_role
            .subject
            .as_ref()
            .is_some_and(|given| *given != subject)
        {
            warnings.push(format!(
                "--subject: pod {pod} is about {subject}, which this member uses; change it for every member with pij pod set {pod} subject=<value>"
            ));
        }
        as_role.subject = Some(subject);
    }
    as_role.fresh |= turn.fresh;
    warnings
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
