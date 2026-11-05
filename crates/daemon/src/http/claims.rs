//! `pij claim`, `pij claims` and `pij delivered` (plan 196, roadmap 001 s11).
//!
//! Claims hand out the next number per (project, kind) with the claiming seat
//! on record; the checkout floor is read in-process, so neither verb launches
//! anything. `pij delivered` is the verb the merger runs once (ruling C23): one
//! gh read, the delivery recorded with one event on the PM's card, and the
//! ddocs edits printed for the PM's next docs PR. It writes no project file.

use std::collections::BTreeSet;
use std::path::Path;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::claims::{self, Address, Edit};
use pij_core::model::{Envelope, ErrorKind, SeatId};
use pij_store::{DeliveryRecord, ReleaseOutcome};
use serde_json::{Value, json};

use super::directory::project_and_repo;
use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};
use crate::community::Refusal;
use crate::community::gh::{E_GH, E_GH_AUTH, E_GH_MISSING, PullRequest};

/// The PR is not merged.
pub const E_NOT_MERGED: &str = "E-RS-PR-NOT-MERGED";
/// The PR body has no usable `Delivers:` line.
pub const E_DELIVERS: &str = "E-RS-DELIVERS";
/// No such claim.
pub const E_CLAIM_UNKNOWN: &str = "E-RS-CLAIM-UNKNOWN";
/// The claim was already released.
pub const E_CLAIM_RELEASED: &str = "E-RS-CLAIM-RELEASED";

/// Flags each verb takes; `--json` is always accepted and ignored here.
const CLAIM_FLAGS: [&str; 2] = ["for", "project"];
const RELEASE_FLAGS: [&str; 2] = ["why", "project"];
const CLAIMS_FLAGS: [&str; 1] = ["project"];
const DELIVERED_FLAGS: [&str; 2] = ["pm", "project"];

/// Kind names that are verbs of `pij claim`, so never claimable.
const RESERVED_KINDS: [&str; 1] = ["release"];

#[derive(Debug, Default)]
struct Arguments {
    positions: Vec<String>,
    flags: std::collections::BTreeMap<String, String>,
    open: bool,
}

impl Arguments {
    fn parse(tokens: &[String], allowed: &[&str], usage: &str) -> Result<Self, Refusal> {
        let mut call = Self::default();
        let mut tokens = tokens.iter();
        while let Some(token) = tokens.next() {
            let Some(flag) = token.strip_prefix("--") else {
                call.positions.push(token.clone());
                continue;
            };
            if flag == "json" {
                continue;
            }
            if flag == "open" && allowed == CLAIMS_FLAGS.as_slice() {
                call.open = true;
                continue;
            }
            let (name, value) = match flag.split_once('=') {
                Some((name, value)) => (name.to_string(), value.to_string()),
                None => (
                    flag.to_string(),
                    tokens
                        .next()
                        .ok_or_else(|| {
                            Refusal::new("E-RS-ARG", format!("--{flag} requires a value"))
                        })?
                        .clone(),
                ),
            };
            if !allowed.contains(&name.as_str()) {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("unsupported --{name}; usage: {usage}"),
                ));
            }
            if call.flags.insert(name.clone(), value).is_some() {
                return Err(Refusal::new("E-RS-ARG", format!("duplicate --{name}")));
            }
        }
        Ok(call)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.flags
            .get(name)
            .filter(|value| !value.trim().is_empty())
            .cloned()
    }
}

/// `POST /v1/claim`, `/v1/claims` and `/v1/delivered`: the verb is argv's first token.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let verb = request.argv.first().cloned().unwrap_or_default();
    let command = format!("pij {verb}");
    if pij_core::directory::wants_help(request.argv.get(1..).unwrap_or_default()) {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(command, json!({"help": claims::HELP})),
        );
    }
    let actor = match resolve_seat(
        &state,
        &command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat.id,
        Resolved::Refusal(response) => return response,
    };
    let args = &request.argv[1..];
    let result = match verb.as_str() {
        "claim" if args.first().is_some_and(|leaf| leaf == "release") => {
            release(&state, &actor, &request.caller, &args[1..]).await
        }
        "claim" => claim(&state, &actor, &request.caller, args).await,
        "claims" => list(&state, &request.caller, args).await,
        "delivered" => delivered(&state, &actor, &request.caller, args).await,
        _ => Err(Refusal::new(
            "E-RS-UNPORTED",
            format!("{verb}: not a claims verb"),
        )),
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(refusal) => refused(&command, &refusal),
    }
}

async fn claim(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    args: &[String],
) -> Result<Value, Refusal> {
    let usage = "pij claim <kind> --for <subject> [--project <slug>]";
    let call = Arguments::parse(args, &CLAIM_FLAGS, usage)?;
    let [kind] = call.positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {usage}")));
    };
    if !claims::valid_kind(kind) || RESERVED_KINDS.contains(&kind.as_str()) {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "{kind:?} is not a claimable kind: lowercase letters, digits and `-`, starting with a letter"
            ),
        ));
    }
    let subject = call.get("for").ok_or_else(|| {
        Refusal::new(
            "E-RS-ARG",
            "--for <subject> is required: it says what the number is for",
        )
    })?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let floor = floor(&repo, kind);
    let at = now()?;
    let claim = state
        .services
        .governance
        .store()
        .claim_number(&project, kind, actor, &subject, floor, at)
        .await
        .map_err(store_refusal)?;
    let formatted = claims::format(kind, claim.number);
    let line = format!(
        "{kind} {formatted} claimed by {actor} for {subject:?} in {project}"
    );
    let mut data = serde_json::to_value(&claim).expect("a claim serialises");
    data["formatted"] = json!(formatted);
    data["floor"] = json!(floor);
    data["line"] = json!(line);
    Ok(data)
}

async fn release(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    args: &[String],
) -> Result<Value, Refusal> {
    let usage = "pij claim release <kind> <number> [--why <text>] [--project <slug>]";
    let call = Arguments::parse(args, &RELEASE_FLAGS, usage)?;
    let [kind, number] = call.positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {usage}")));
    };
    let number: u64 = number
        .parse()
        .map_err(|_| Refusal::new("E-RS-ARG", format!("{number:?} is not a number")))?;
    let (project, _) = project_and_repo(state, caller, call.get("project")).await?;
    let prime = project_prime(state, &project).await?;
    let reason = call
        .get("why")
        .unwrap_or_else(|| "released by hand".to_string());
    let outcome = state
        .services
        .governance
        .store()
        .release_claim(&project, kind, number, actor, prime.as_ref(), &reason, now()?)
        .await
        .map_err(store_refusal)?;
    let named = format!("{kind} {} in {project}", claims::format(kind, number));
    match outcome {
        ReleaseOutcome::Released(claim) => {
            let mut data = serde_json::to_value(&claim).expect("a claim serialises");
            data["line"] = json!(format!("{named} released by {actor}"));
            Ok(data)
        }
        ReleaseOutcome::Unknown => Err(Refusal::new(
            E_CLAIM_UNKNOWN,
            format!("no claim for {named}"),
        )
        .with_next(format!("List them: pij claims --project {project}"))),
        ReleaseOutcome::AlreadyReleased(claim) => Err(Refusal::new(
            E_CLAIM_RELEASED,
            format!(
                "{named} was already released ({})",
                claim.release_reason.unwrap_or_default()
            ),
        )),
        ReleaseOutcome::NotOwner(claim) => Err(Refusal::new(
            "E-RS-OWNERSHIP",
            format!(
                "{named} is held by {}; only that seat or the prime releases it",
                claim.seat
            ),
        )),
    }
}

async fn list(state: &AppState, caller: &CallerContext, args: &[String]) -> Result<Value, Refusal> {
    let usage = "pij claims [--project <slug>] [--open]";
    let call = Arguments::parse(args, &CLAIMS_FLAGS, usage)?;
    if !call.positions.is_empty() {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {usage}")));
    }
    let (project, _) = project_and_repo(state, caller, call.get("project")).await?;
    let rows = state
        .services
        .governance
        .store()
        .claims(&project)
        .await
        .map_err(store_refusal)?;
    let claims: Vec<Value> = rows
        .iter()
        .filter(|claim| !call.open || claim.released_at.is_none())
        .map(|claim| {
            let mut row = serde_json::to_value(claim).expect("a claim serialises");
            row["formatted"] = json!(claims::format(&claim.kind, claim.number));
            row
        })
        .collect();
    let open = rows.iter().filter(|claim| claim.released_at.is_none()).count();
    Ok(json!({"project": project, "open": open, "claims": claims}))
}

async fn delivered(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    args: &[String],
) -> Result<Value, Refusal> {
    let usage = "pij delivered <pr> [--pm <seat>] [--project <slug>]";
    let call = Arguments::parse(args, &DELIVERED_FLAGS, usage)?;
    let [pr] = call.positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {usage}")));
    };
    let number: u64 = pr
        .trim_start_matches('#')
        .parse()
        .map_err(|_| Refusal::new("E-RS-ARG", format!("{pr:?} is not a PR number")))?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let flag_pm = match call.get("pm") {
        Some(seat) => Some(live_seat(state, &seat).await?.ok_or_else(|| {
            Refusal::new(
                "E-RS-NOT-FOUND",
                format!("--pm {seat}: no live seat by that name"),
            )
        })?),
        None => None,
    };

    let gh = std::sync::Arc::clone(&state.services.gh);
    let read_from = repo.clone();
    let pull = tokio::task::spawn_blocking(move || gh.view(&read_from, number))
        .await
        .map_err(|error| Refusal::new(E_GH, format!("the gh read failed: {error}")))??;
    if !pull.merged() {
        return Err(Refusal::new(
            E_NOT_MERGED,
            format!("PR #{number} is {}, not merged", pull.state),
        )
        .with_next("Run `pij delivered` after the merge, once."));
    }
    let merge_commit = pull
        .merge_commit
        .clone()
        .ok_or_else(|| Refusal::new(E_GH, format!("PR #{number} is merged but gh named no merge commit")))?;
    let addresses = claims::delivers(&pull.body)
        .map_err(|error| Refusal::new(E_DELIVERS, format!("PR #{number}: {error}")))?;

    let (pm, pm_source) = match holder_pm(state, &project, &addresses).await? {
        Some(holder) => (Some(holder), "holder"),
        None => match flag_pm {
            Some(seat) => (Some(seat), "flag"),
            None => (None, "none"),
        },
    };
    let mut notes = Vec::new();
    if let (Some(flag), Some(told)) = (call.get("pm"), &pm)
        && pm_source == "holder"
        && flag != told.as_str()
    {
        notes.push(format!(
            "--pm {flag} not used: {told} holds the PM role for this roadmap"
        ));
    }
    let releases: Vec<(String, u64)> = claims::numbers_in_paths(pull.files.iter().map(String::as_str))
        .into_iter()
        .collect();
    let edits = edits(&repo, &pull, &addresses, &mut notes);

    let record = DeliveryRecord {
        project: project.clone(),
        pr: number,
        merge_commit: merge_commit.clone(),
        recorded_by: actor.to_string(),
        pm: pm.as_ref().map(ToString::to_string),
        addresses: addresses.iter().map(ToString::to_string).collect(),
        releases,
        at: now()?,
    };
    let store = state.services.governance.store().clone();
    let commit = state
        .services
        .event_bus
        .publish_committed_batch(async move {
            let commit = store.record_delivery_committed(&record).await?;
            Ok((commit.events.clone(), commit))
        })
        .await
        .map_err(store_refusal)?;

    let told = match (&pm, commit.events.first()) {
        (Some(pm), Some(_)) => format!("posted on {pm}'s card"),
        (Some(pm), None) => format!("already recorded; {pm}'s card was told the first time"),
        (None, _) => "no PM was told: no live pm holder for this roadmap and no --pm".to_string(),
    };
    let line = format!(
        "PR #{number} ({}) delivered {} address(es) in {project}: {} new, {} already recorded; {} claim(s) released; {told}",
        merge_commit.get(..12).unwrap_or(&merge_commit),
        addresses.len(),
        commit.recorded.len(),
        commit.already.len(),
        commit.released.len(),
    );
    Ok(json!({
        "project": project,
        "pr": number,
        "url": pull.url,
        "merge_commit": merge_commit,
        "addresses": addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "recorded": commit.recorded,
        "already": commit.already,
        "released": commit.released,
        "pm": pm,
        "pm_source": pm_source,
        "earlier_pm": commit.earlier_pm,
        "event_seq": commit.events.first().and_then(|event| event.seq).map(|seq| seq.0),
        "edits": edits,
        "notes": notes,
        "line": line,
    }))
}

/// The live holder of the project's `pm-<roadmap>` role for the roadmaps the
/// addresses' steps belong to, when exactly one seat holds them.
async fn holder_pm(
    state: &AppState,
    project: &str,
    addresses: &[Address],
) -> Result<Option<SeatId>, Refusal> {
    let roles: BTreeSet<String> = addresses
        .iter()
        .filter_map(Address::roadmap)
        .map(|roadmap| format!("pm-{}", claims::format("roadmap", roadmap)))
        .collect();
    if roles.is_empty() {
        return Ok(None);
    }
    let holdings = state
        .services
        .governance
        .store()
        .assignments(project)
        .await
        .map_err(store_refusal)?;
    let mut holders = BTreeSet::new();
    for holding in holdings
        .iter()
        .filter(|holding| holding.to.is_none() && roles.contains(&holding.role))
    {
        if let Some(seat) = live_seat(state, &holding.seat).await? {
            holders.insert(seat);
        }
    }
    Ok(match holders.len() {
        1 => holders.into_iter().next(),
        _ => None,
    })
}

async fn live_seat(state: &AppState, seat: &str) -> Result<Option<SeatId>, Refusal> {
    let id = SeatId::from(seat);
    let found = state
        .services
        .registry
        .get(&id)
        .await
        .map_err(store_refusal)?;
    Ok(found
        .filter(|descriptor| descriptor.tombstoned_at.is_none())
        .map(|descriptor| descriptor.id))
}

/// The `ddocs set` edits for the PM's next docs PR: each requirement's
/// `delivered_by` gains this PR, and each step's `plan` names the PR's plan
/// folder. Current values are read in-process from the project's checkout.
fn edits(repo: &Path, pull: &PullRequest, addresses: &[Address], notes: &mut Vec<String>) -> Vec<Edit> {
    let plans = claims::plan_folders(pull.files.iter().map(String::as_str));
    let mut edits = Vec::new();
    for address in addresses {
        let document = std::fs::read_to_string(repo.join(&address.path))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        if document.is_none() {
            notes.push(format!(
                "{}: could not read it in {}; the edit assumes the cell is empty",
                address.path,
                repo.display()
            ));
        }
        let current = |field: &str| {
            document
                .as_ref()
                .and_then(|document| claims::row_field(document, &address.section, &address.id, field))
        };
        match address.section.as_str() {
            "requirements" => {
                match claims::delivered_by(current("delivered_by").as_deref(), pull.number) {
                    Some(value) => edits.push(Edit::set(address, "delivered_by", value)),
                    None => notes.push(format!("{address}: delivered_by already names #{}", pull.number)),
                }
            }
            "steps" => match plans.iter().collect::<Vec<_>>().as_slice() {
                [plan] if current("plan").as_deref() == Some(plan.as_str()) => {}
                [plan] => edits.push(Edit::set(address, "plan", (*plan).clone())),
                [] => notes.push(format!("{address}: the PR touches no docs/plans/NNN-* folder; set its plan by hand")),
                _ => notes.push(format!(
                    "{address}: the PR touches several plan folders ({}); set its plan by hand",
                    plans.iter().cloned().collect::<Vec<_>>().join(", ")
                )),
            },
            _ => notes.push(format!("{address}: recorded; no edit is printed for a {} row", address.section)),
        }
    }
    edits
}

/// The highest number of `kind` already in the project's main checkout, read
/// in-process (no launch); 0 when there is none or the kind has no folder.
fn floor(repo: &Path, kind: &str) -> u64 {
    claims::folders(kind)
        .iter()
        .filter_map(|folder| std::fs::read_dir(repo.join(folder)).ok())
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| claims::number_in_name(kind, &entry.file_name().to_string_lossy()))
        .max()
        .unwrap_or(0)
}

async fn project_prime(state: &AppState, project: &str) -> Result<Option<SeatId>, Refusal> {
    Ok(state
        .services
        .governance
        .store()
        .list_projects()
        .await
        .map_err(store_refusal)?
        .into_iter()
        .find(|candidate| candidate.slug == project)
        .and_then(|project| project.prime_id))
}

fn now() -> Result<u64, Refusal> {
    system_time_ms().map_err(store_refusal)
}

fn store_refusal(error: pij_core::error::PijError) -> Refusal {
    Refusal::new("E-RS-STORE", error.to_string())
}

fn refused(command: &str, refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-ARG" | "E-RS-UNPORTED" | E_DELIVERS => StatusCode::BAD_REQUEST,
        "E-RS-OWNERSHIP" => StatusCode::FORBIDDEN,
        "E-RS-NOT-FOUND" | E_CLAIM_UNKNOWN => StatusCode::NOT_FOUND,
        "E-RS-STORE" | E_GH | E_GH_MISSING | E_GH_AUTH => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    let mut body = Envelope::<Value>::refused(
        command,
        if status.is_server_error() {
            ErrorKind::Adapter
        } else {
            ErrorKind::Refused
        },
        refusal.to_string(),
    );
    body.details = Some(json!({"code": refusal.code, "next": refusal.next}));
    envelope(status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    #[test]
    fn arguments_take_their_verbs_flags_and_refuse_others() {
        let call = Arguments::parse(
            &argv(&["plan", "--for", "claims", "--project=pij", "--json"]),
            &CLAIM_FLAGS,
            "u",
        )
        .unwrap();
        assert_eq!(call.positions, ["plan"]);
        assert_eq!(call.get("for").as_deref(), Some("claims"));
        assert_eq!(call.get("project").as_deref(), Some("pij"));
        assert_eq!(
            Arguments::parse(&argv(&["plan", "--pm", "x"]), &CLAIM_FLAGS, "u")
                .unwrap_err()
                .code,
            "E-RS-ARG"
        );
        assert_eq!(
            Arguments::parse(&argv(&["plan", "--for"]), &CLAIM_FLAGS, "u")
                .unwrap_err()
                .message,
            "--for requires a value"
        );
        assert!(Arguments::parse(&argv(&["--open"]), &CLAIMS_FLAGS, "u").unwrap().open);
        assert!(Arguments::parse(&argv(&["--open"]), &CLAIM_FLAGS, "u").is_err());
    }

    #[test]
    fn the_floor_is_the_highest_number_in_the_kinds_folders() {
        let root = pij_testkit::fresh_dir("pij-claims-floor");
        for dir in [
            "docs/plans/190-a",
            "docs/plans/archive/195-b",
            "docs/plans/notes",
            "crates/store/migrations",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("crates/store/migrations/0032_x.sql"), "").unwrap();
        assert_eq!(floor(&root, "plan"), 195);
        assert_eq!(floor(&root, "migration"), 32);
        assert_eq!(floor(&root, "prd"), 0);
        assert_eq!(floor(&root, "adr"), 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
