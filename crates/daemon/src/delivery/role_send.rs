//! Sending to a role (plan 199, roadmap 001 s05: req-07 and the request half
//! of req-08).
//!
//! `pij send role:<id>[/<instance>]` resolves through the project's directory
//! (ruling C1), never through `seat_roles`: one live holder gets an ordinary
//! send, recorded as a delivered request; none queues the request and wakes the
//! project's PA (else the prime) once, with runnable next steps; several refuse
//! with their names. An assign drains the queue to its new holder (C6). Every
//! request carries the requester's reply mode and limit for s06 and s12.
//!
//! Cost: what a normal send costs, plus in-process reads (the governance
//! worktree is found without launching git, the role document is one file
//! read) and one store write. A queued request adds one wake delivery; a drain
//! one delivery per request. No timer, poll or launch site.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pij_core::cold_wake::{human_duration, human_tokens};
use pij_core::model::{DeliveryOutcome, Msg, Receipt, SeatDescriptor, SeatId};
use pij_core::orchestration::PrimeState;
use pij_core::ports::Registry;
use pij_core::session_status::SessionStatusBlock;
use pij_store::{Assignment, ReplyMode, RequestState, RoleRequest, SqliteOrchestration};
use serde_json::{Value, json};

use crate::Services;
use crate::community::directory::{AfterAssign, E_ROLE_MISSING};
use crate::community::holders::seat_role_for;
use crate::community::layout::RolePaths;
use crate::community::{Refusal, root};

/// Every role address starts with this.
pub const ROLE_PREFIX: &str = "role:";
/// A role address that is malformed, remote, or names an instance of a
/// one-holder role.
pub const E_ROLE_ADDRESS: &str = "E-RS-ROLE-ADDRESS";
/// Two or more live holders: the send cannot pick one.
pub const E_ROLE_HOLDERS: &str = "E-RS-ROLE-HOLDERS";
/// The directory and `seat_roles` disagree about the holder (ruling C1).
pub const E_ROLE_MISMATCH: &str = "E-RS-ROLE-MISMATCH";
/// `--reply` or `--limit` on a send that is not to a role.
pub const E_ROLE_ONLY: &str = "E-RS-ROLE-ONLY";
/// A msg_id that already names a different request.
pub const E_ROLE_REQUEST_EXISTS: &str = "E-RS-ROLE-REQUEST-EXISTS";

/// How long the last holder's context size may take to read.
const STATUS_WAIT: Duration = Duration::from_secs(3);

/// A parsed `role:<id>[/<instance>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleAddress {
    /// The role id.
    pub role: String,
    /// The instance, or empty for any holder.
    pub instance: String,
}

impl RoleAddress {
    /// `None` when `to` is not a role address at all.
    pub fn parse(to: &str) -> Option<Result<Self, Refusal>> {
        let rest = to.strip_prefix(ROLE_PREFIX)?;
        let (role, instance) = rest.split_once('/').unwrap_or((rest, ""));
        let word = |text: &str| {
            !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if !word(role) || (rest.contains('/') && !word(instance)) {
            return Some(Err(Refusal::new(
                E_ROLE_ADDRESS,
                format!("`{to}` is not role:<id> or role:<id>/<instance>"),
            )));
        }
        Some(Ok(Self {
            role: role.to_string(),
            instance: instance.to_string(),
        }))
    }

    /// `role:<id>` or `role:<id>/<instance>`.
    pub fn display(&self) -> String {
        if self.instance.is_empty() {
            format!("{ROLE_PREFIX}{}", self.role)
        } else {
            format!("{ROLE_PREFIX}{}/{}", self.role, self.instance)
        }
    }
}

/// What a role send carries besides the message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleOptions {
    /// `--reply wake|fyi|none`.
    pub reply: Option<String>,
    /// `--limit <duration>`.
    pub limit: Option<String>,
}

impl RoleOptions {
    /// Refuse the options on a send that is not to a role.
    pub fn refuse_on_seat_send(&self) -> Result<(), Refusal> {
        match (&self.reply, &self.limit) {
            (None, None) => Ok(()),
            _ => Err(Refusal::new(
                E_ROLE_ONLY,
                "--reply and --limit apply to a send to role:<id>; a send to a seat has neither",
            )),
        }
    }
}

/// One role send, after the caller resolved its project.
#[derive(Clone, Debug)]
pub struct RoleSend {
    /// Who sent it.
    pub from: SeatId,
    /// The address.
    pub address: RoleAddress,
    /// The project whose directory holds the role.
    pub project: String,
    /// That project's repository, where its governance worktree is found.
    pub repo: std::path::PathBuf,
    /// The body as written.
    pub body: String,
    /// The send's msg_id: the request id.
    pub msg_id: String,
    /// The message it answers.
    pub in_reply_to: Option<String>,
    /// `--fyi`.
    pub fyi: bool,
    /// `--reply` and `--limit`.
    pub options: RoleOptions,
}

/// Where a role send goes.
#[derive(Debug)]
pub enum Routed {
    /// One holder: record `request` (claimed for `seat`, unless `recorded`),
    /// send `body` to `seat` the ordinary way, then mark it delivered, or
    /// abandon the claim when the send does not go through.
    Holder {
        /// The holder.
        seat: SeatId,
        /// The body with its request header.
        body: String,
        /// The request, `delivering` to `seat`.
        request: RoleRequest,
        /// Already recorded: a send interrupted after its claim resumes to
        /// the holder it claimed, never a new one.
        recorded: bool,
    },
    /// A retry of a request already queued or delivered: nothing is sent
    /// again, and the stored request is the answer.
    Replay {
        /// The stored request.
        request: RoleRequest,
    },
    /// No live holder: the request is queued and the wake was attempted.
    Queued {
        /// The receipt for the sender.
        receipt: Receipt,
        /// The queued request.
        request: RoleRequest,
        /// Who was woken, and how it went.
        woke: Value,
    },
}

/// Resolve a role send: refuse it, route it to its one holder, or queue it.
///
/// # Errors
/// A [`Refusal`] naming what is wrong: the address, the role, the options, two
/// or more holders, or a C1 mismatch. Nothing is recorded or sent.
pub async fn route(services: &Services, send: RoleSend, now_ms: u64) -> Result<Routed, Refusal> {
    let store = services.governance.store().clone();
    // A request id is admitted once (review of #129): a retry is answered from
    // the stored request before any holder is resolved, so a rotation between
    // the two can never send it to a second holder.
    if let Some(existing) = store
        .role_request(&send.msg_id)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
    {
        return replay(&send, existing);
    }
    let document = role_document(&send.repo, &send.project, &send.address.role)?;
    let meta = section(&document, "meta");
    if !send.address.instance.is_empty()
        && meta.and_then(|meta| meta["holders"].as_str()) == Some("one")
    {
        return Err(Refusal::new(
            E_ROLE_ADDRESS,
            format!(
                "role {} has one holder and no instances; address it as {ROLE_PREFIX}{}",
                send.address.role, send.address.role
            ),
        ));
    }
    let reply = match send.options.reply.as_deref() {
        None => ReplyMode::Wake,
        Some(text) => ReplyMode::parse(text).ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!("--reply takes wake, fyi or none, not `{text}`"),
            )
        })?,
    };
    let limit_ms = match send.options.limit.as_deref() {
        Some(text) => Some(
            crate::http::background::parse_duration_ms(text, "--limit")
                .map_err(|message| Refusal::new("E-RS-ARG", message))?,
        ),
        // C5: the role's own default, when it states a readable one.
        None => meta
            .and_then(|meta| meta["default_limit"].as_str())
            .and_then(|text| {
                crate::http::background::parse_duration_ms(text, "default_limit").ok()
            }),
    };
    let assignments = store
        .assignments(&send.project)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    let mut live = Vec::new();
    for row in assignments.iter().filter(|row| {
        row.to.is_none()
            && row.role == send.address.role
            && (send.address.instance.is_empty() || row.instance == send.address.instance)
    }) {
        if let Some(seat) = live_seat(services.registry.as_ref(), &row.seat).await? {
            live.push((row, seat));
        }
    }
    let mut request = RoleRequest {
        id: send.msg_id.clone(),
        project: send.project.clone(),
        role: send.address.role.clone(),
        instance: send.address.instance.clone(),
        requester: send.from.to_string(),
        body: send.body.clone(),
        in_reply_to: send.in_reply_to.clone(),
        fyi: send.fyi,
        reply,
        limit_ms,
        state: RequestState::Queued,
        holder: None,
        created_at: now_ms,
        delivered_at: None,
        checkin_at: None,
        escalated_at: None,
        answered_at: None,
    };
    match live.as_slice() {
        [] => {
            if let Err(error) = store.record_role_request(&request).await {
                // An identical send recorded it first: answer from that one.
                if let Ok(Some(existing)) = store.role_request(&request.id).await {
                    return replay(&send, existing);
                }
                return Err(Refusal::new("E-RS-STORE", error.to_string()));
            }
            let woke = wake(services, &store, &request, &assignments, now_ms).await;
            let receipt = Receipt {
                msg_id: request.id.clone(),
                outcome: DeliveryOutcome::Queued {
                    reason: Some(format!(
                        "no live holder of {} in project {}: queued as request {} until a seat is assigned",
                        send.address.display(),
                        send.project,
                        request.id
                    )),
                    next_retry_at: None,
                    draft_sha: None,
                },
                at: now_ms,
                cold_check: None,
                warning: None,
            };
            Ok(Routed::Queued {
                receipt,
                request,
                woke,
            })
        }
        [(row, seat)] => {
            check_seat_role(&store, &send.address.role, &seat.id).await?;
            request.state = RequestState::Delivering;
            request.holder = Some(seat.id.to_string());
            request.instance.clone_from(&row.instance);
            Ok(Routed::Holder {
                seat: seat.id.clone(),
                body: framed_body(&request),
                request,
                recorded: false,
            })
        }
        several => Err(Refusal::new(
            E_ROLE_HOLDERS,
            format!(
                "{} has {} live holders in project {}: {}; address one as {ROLE_PREFIX}{}/<instance>, or send to the seat",
                send.address.display(),
                several.len(),
                send.project,
                several
                    .iter()
                    .map(|(row, seat)| if row.instance.is_empty() {
                        seat.id.to_string()
                    } else {
                        format!("{} ({})", seat.id, row.instance)
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                send.address.role
            ),
        )),
    }
}

/// Answer a send whose msg_id is already a request: the same request again
/// is a replay; anything else reusing the id refuses by name.
fn replay(send: &RoleSend, existing: RoleRequest) -> Result<Routed, Refusal> {
    let same = existing.project == send.project
        && existing.role == send.address.role
        && (send.address.instance.is_empty() || existing.instance == send.address.instance)
        && existing.requester == send.from.as_str()
        && existing.body == send.body
        && existing.fyi == send.fyi;
    if !same {
        return Err(Refusal::new(
            E_ROLE_REQUEST_EXISTS,
            format!(
                "msg_id {} already names a different request ({} in {} from {}); send this one with a new msg_id",
                existing.id,
                RoleAddress {
                    role: existing.role.clone(),
                    instance: existing.instance.clone(),
                }
                .display(),
                existing.project,
                existing.requester
            ),
        ));
    }
    match (existing.state, existing.holder.clone()) {
        (RequestState::Delivering, Some(holder)) => Ok(Routed::Holder {
            seat: SeatId::from(holder.as_str()),
            body: framed_body(&existing),
            request: existing,
            recorded: true,
        }),
        _ => Ok(Routed::Replay { request: existing }),
    }
}

/// The receipt for a replay: nothing was sent again.
pub fn replay_receipt(request: &RoleRequest, now_ms: u64) -> Receipt {
    let reason = match (request.state, request.holder.as_deref()) {
        (RequestState::Delivered, Some(holder)) => format!(
            "replay: request {} was already delivered to {holder}; not sent again",
            request.id
        ),
        _ => format!(
            "replay: request {} is still queued for a holder; not sent again",
            request.id
        ),
    };
    let outcome = if request.state == RequestState::Delivered {
        DeliveryOutcome::Held { reason }
    } else {
        DeliveryOutcome::Queued {
            reason: Some(reason),
            next_retry_at: None,
            draft_sha: None,
        }
    };
    Receipt {
        msg_id: request.id.clone(),
        outcome,
        at: now_ms,
        cold_check: None,
        warning: None,
    }
}

/// The request as the receipt shows it.
pub fn request_json(request: &RoleRequest) -> Value {
    json!({
        "id": request.id,
        "project": request.project,
        "role": request.role,
        "instance": request.instance,
        "reply": request.reply.as_str(),
        "limit_ms": request.limit_ms,
        "state": request.state.as_str(),
        "holder": request.holder,
    })
}

/// The body a holder receives: one header line naming the role, the request
/// and how the requester wants to hear back, then the body as written.
pub fn framed_body(request: &RoleRequest) -> String {
    let address = RoleAddress {
        role: request.role.clone(),
        instance: request.instance.clone(),
    }
    .display();
    let limit = request
        .limit_ms
        .map_or_else(String::new, |ms| format!(" · limit {}", human_duration(ms)));
    format!(
        "[pij {address} request {} from {} · reply {}{limit}]\n{}",
        request.id,
        request.requester,
        request.reply.as_str(),
        request.body
    )
}

/// The role's document in the project's governance worktree, found without
/// launching anything.
fn role_document(repo: &Path, project: &str, role: &str) -> Result<Value, Refusal> {
    let root = root::find(repo)?;
    let paths = RolePaths::new(role)?;
    let path = root.worktree.join(&paths.document);
    let missing = || {
        Refusal::new(
            E_ROLE_MISSING,
            format!("project {project} has no role {role} in its directory"),
        )
        .with_next(format!("pij directory view --project {project}"))
    };
    let text = std::fs::read_to_string(&path).map_err(|_| missing())?;
    serde_json::from_str(&text).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("{} is not a role document: {error}", path.display()),
        )
    })
}

fn section<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == name)
        .map(|section| &section["value"])
}

async fn live_seat(registry: &dyn Registry, seat: &str) -> Result<Option<SeatDescriptor>, Refusal> {
    let row = registry
        .get(&SeatId::from(seat))
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    Ok(row.filter(|row| row.tombstoned_at.is_none()))
}

/// Ruling C1: the directory decides, and an assign keeps `seat_roles` in step.
/// A holder whose recorded role says otherwise refuses by name rather than
/// receiving work its role does not match. A prime is never re-roled, and a
/// seat with no recorded role has nothing to disagree with.
async fn check_seat_role(
    store: &SqliteOrchestration,
    role: &str,
    seat: &SeatId,
) -> Result<(), Refusal> {
    let expected = seat_role_for(role);
    let recorded = store
        .list_roles()
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        .into_iter()
        .find(|row| &row.seat == seat)
        .map(|row| row.role);
    match recorded.as_deref() {
        Some(actual) if actual != expected && actual != "prime" => Err(Refusal::new(
            E_ROLE_MISMATCH,
            format!(
                "{seat} holds {role} in the directory, which makes it a {expected}, but seat_roles records it as {actual}"
            ),
        )
        .with_next(format!("pij role {seat} {expected}"))),
        _ => Ok(()),
    }
}

/// Wake the project's PA, else the prime, once for a queued request.
async fn wake(
    services: &Services,
    store: &SqliteOrchestration,
    request: &RoleRequest,
    assignments: &[Assignment],
    now_ms: u64,
) -> Value {
    let target = match wake_target(services, store, assignments).await {
        Ok(Some(target)) => target,
        Ok(None) => return json!({"seat": null, "outcome": "no live PA or prime to wake"}),
        Err(cause) => return json!({"seat": null, "outcome": format!("error: {cause}")}),
    };
    let body = wake_body(services, request, assignments, now_ms).await;
    let (outcome, cold_check) = crate::pa_watchdog::wake_pa(
        services,
        &target,
        body,
        format!("role-queue-{}", request.id),
        "role-queue",
        now_ms,
    )
    .await;
    json!({"seat": target.id, "outcome": outcome, "cold_check": cold_check})
}

/// The project's live `pa` holder; else the current prime designation; else a
/// live seat with the `prime` role. Ties go to the lowest seat id.
async fn wake_target(
    services: &Services,
    store: &SqliteOrchestration,
    assignments: &[Assignment],
) -> Result<Option<SeatDescriptor>, Refusal> {
    let store_error =
        |error: pij_core::error::PijError| Refusal::new("E-RS-STORE", error.to_string());
    let mut pas = Vec::new();
    for row in assignments
        .iter()
        .filter(|row| row.to.is_none() && row.role == "pa")
    {
        if let Some(seat) = live_seat(services.registry.as_ref(), &row.seat).await? {
            pas.push(seat);
        }
    }
    if let Some(pa) = pas.into_iter().min_by(|a, b| a.id.cmp(&b.id)) {
        return Ok(Some(pa));
    }
    if let Some(designation) = store
        .prime()
        .await
        .map_err(store_error)?
        .filter(|designation| designation.state == PrimeState::Current)
        && let Some(seat) = live_seat(services.registry.as_ref(), designation.seat.as_str()).await?
    {
        return Ok(Some(seat));
    }
    let mut primes = Vec::new();
    for row in store
        .list_roles()
        .await
        .map_err(store_error)?
        .into_iter()
        .filter(|row| row.role == "prime")
    {
        if let Some(seat) = live_seat(services.registry.as_ref(), row.seat.as_str()).await? {
            primes.push(seat);
        }
    }
    Ok(primes.into_iter().min_by(|a, b| a.id.cmp(&b.id)))
}

/// The wake: the request, and the two ways to give the role a holder.
async fn wake_body(
    services: &Services,
    request: &RoleRequest,
    assignments: &[Assignment],
    now_ms: u64,
) -> String {
    let address = RoleAddress {
        role: request.role.clone(),
        instance: request.instance.clone(),
    };
    let assign_target = if request.instance.is_empty() {
        request.role.clone()
    } else {
        format!("{}/{}", request.role, request.instance)
    };
    let mut text = format!(
        "[pij role queue] {} in project {} has no live holder. {} sent request {} (reply {}{}); it waits on the role's queue.\nNext, one of:",
        address.display(),
        request.project,
        request.requester,
        request.id,
        request.reply.as_str(),
        request
            .limit_ms
            .map_or_else(String::new, |ms| format!(", limit {}", human_duration(ms))),
    );
    if let Some(last) = last_holder(assignments, request) {
        let context = context_tokens(services, &last, now_ms).await.map_or_else(
            || "context unknown".to_string(),
            |tokens| format!("context {}", human_tokens(tokens)),
        );
        text.push_str(&format!(
            "\n  revive the last holder {last} ({context}): pij-rs revive {last}"
        ));
    }
    text.push_str(&format!(
        "\n  assign a fresh seat: pij directory assign {assign_target} <seat> --why \"<why>\" --project {}",
        request.project
    ));
    text.push_str(
        "\nAssigning delivers the queue to the new holder. pij never starts a seat itself.",
    );
    text
}

/// The seat that held the addressed role (and instance) most recently.
fn last_holder(assignments: &[Assignment], request: &RoleRequest) -> Option<SeatId> {
    assignments
        .iter()
        .filter(|row| {
            row.role == request.role
                && (request.instance.is_empty() || row.instance == request.instance)
        })
        .max_by(|a, b| {
            a.to.as_deref()
                .unwrap_or(&a.from)
                .cmp(b.to.as_deref().unwrap_or(&b.from))
        })
        .map(|row| SeatId::from(row.seat.as_str()))
}

/// The seat's context size from its session facts, as the cold-wake verdict
/// reads them, bounded by the same wait.
async fn context_tokens(services: &Services, seat: &SeatId, now_ms: u64) -> Option<u64> {
    let descriptor = services.registry.get(seat).await.ok()??;
    let block = tokio::time::timeout(
        STATUS_WAIT,
        crate::http::session_status_block(
            services.session_status.as_ref(),
            &descriptor.id,
            descriptor.harness,
            descriptor.harness_session.clone(),
            now_ms,
        ),
    )
    .await
    .ok()?;
    match block {
        SessionStatusBlock::Known { status, .. } => status.context_used_tokens.value().copied(),
        _ => None,
    }
}

/// Why a drained request may wake a cold holder: the assign that triggered the
/// drain is the deliberate act, and a refused wake would strand the request
/// until the next assign. The wake is audited before it is sent, as every
/// daemon-initiated wake is.
pub const DRAIN_WAKE_REASON: &str = "role queue: drained to the role's new holder on assign";

/// Ruling C6: after an assign lands, deliver the role's queued requests to the
/// new holder, in order. The hook is synchronous, so the drain runs on its own
/// task; an assign that was refused never calls it.
///
/// The drain needs the built [`Services`] (cold-wake verdicts, the audit bus),
/// which own the directory that holds this hook, so they are bound once after
/// they are built ([`RoleQueueDrain::bind`]). Before `bind`, an assign drains
/// nothing and the requests stay queued.
#[derive(Default)]
pub struct RoleQueueDrain {
    services: std::sync::OnceLock<Services>,
}

impl RoleQueueDrain {
    /// An unbound drain; see [`RoleQueueDrain::bind`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain through these services from now on. A second bind is ignored.
    /// The drain keeps them with a hookless copy of the directory, so the
    /// directory holding this hook is not held back by it: no reference cycle,
    /// and a stopped daemon's services drop.
    pub fn bind(&self, mut services: Services) {
        services.directory = Arc::new(services.directory.without_hook());
        let _ = self.services.set(services);
    }
}

impl AfterAssign for RoleQueueDrain {
    fn assigned(&self, project: &str, role: &str, instance: &str, seat: &SeatId) {
        let Some(services) = self.services.get().cloned() else {
            eprintln!("pij-rs: role queue for {role} not drained: services not bound");
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            eprintln!("pij-rs: role queue for {role} not drained: no runtime");
            return;
        };
        let (project, role, instance, seat) = (
            project.to_string(),
            role.to_string(),
            instance.to_string(),
            seat.clone(),
        );
        runtime.spawn(async move {
            for (id, outcome) in drain(&services, &project, &role, &instance, &seat).await {
                if let Err(cause) = outcome {
                    eprintln!("pij-rs: role request {id} for {seat} not drained: {cause}");
                }
            }
        });
    }
}

/// Deliver `role`'s queued requests a holder of `instance` can take to `seat`,
/// oldest first, each from its requester; mark each one delivered.
///
/// Each is claimed before it is dispatched, and the claim holds only while
/// `seat` is live and holds the role at that moment, so two drains never
/// deliver one request and a drain scheduled before a release delivers nothing
/// (reviews of #129: F002, security S1). A non-FYI delivery is admitted as an
/// immediate send is (security S2): a cold holder's wake is audited first, as
/// a forced send from the requester with [`DRAIN_WAKE_REASON`], and a failed
/// audit sends nothing. An FYI is held, never a wake. One that is not
/// delivered goes back on the queue for the next assign.
pub async fn drain(
    services: &Services,
    project: &str,
    role: &str,
    instance: &str,
    seat: &SeatId,
) -> Vec<(String, Result<(), String>)> {
    let store = services.directory.store();
    let queued = match store.queued_role_requests(project, role, instance).await {
        Ok(queued) => queued,
        Err(cause) => return vec![(format!("{project}/{role}"), Err(cause.to_string()))],
    };
    let mut outcomes = Vec::new();
    for mut request in queued {
        match store.claim_role_request(&request.id, seat.as_str()).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(cause) => {
                outcomes.push((request.id, Err(cause.to_string())));
                continue;
            }
        }
        request.instance = instance.to_string();
        let outcome = deliver_drained(services, &request, seat).await;
        let outcome = match outcome {
            Ok(at) => store
                .mark_role_request_delivered(&request.id, seat.as_str(), at)
                .await
                .map(|_| ())
                .map_err(|cause| cause.to_string()),
            Err(cause) => Err(cause),
        };
        if outcome.is_err() {
            // Back on the queue for the next assign.
            let _ = store
                .release_role_request_claim(&request.id, seat.as_str())
                .await;
        }
        outcomes.push((request.id, outcome));
    }
    outcomes
}

/// At boot, settle the requests a stopped daemon left `delivering` (review
/// of #129): one delivery already took is marked delivered, any other is
/// re-queued for the next assign. A failure is logged; boot carries on.
pub async fn recover_requests(services: &Services) {
    let now_ms = match crate::http::system_time_ms() {
        Ok(now) => now,
        Err(cause) => {
            eprintln!("pij-rs: role requests not recovered: {cause}");
            return;
        }
    };
    match services
        .directory
        .store()
        .recover_delivering_role_requests(now_ms)
        .await
    {
        Ok((0, 0)) => {}
        Ok((delivered, requeued)) => eprintln!(
            "pij-rs: role requests left delivering: {delivered} marked delivered, {requeued} re-queued"
        ),
        Err(cause) => eprintln!("pij-rs: role requests not recovered: {cause}"),
    }
}

/// One claimed request to `seat`, through the admission an immediate send
/// gets. The delivery time, or why nothing was delivered.
async fn deliver_drained(
    services: &Services,
    request: &RoleRequest,
    seat: &SeatId,
) -> Result<u64, String> {
    let msg = Msg {
        from: SeatId::from(request.requester.as_str()),
        from_machine: None,
        to: seat.clone(),
        body: framed_body(request),
        msg_id: request.id.clone(),
        in_reply_to: request.in_reply_to.clone(),
        command: None,
    };
    let sent = if request.fyi {
        services.delivery.hold_fyi(msg).await
    } else {
        let descriptor = match services.registry.get(seat).await {
            Ok(Some(descriptor)) if descriptor.tombstoned_at.is_none() => descriptor,
            Ok(_) => return Err(format!("{seat} is no longer live")),
            Err(cause) => return Err(cause.to_string()),
        };
        let now_ms = crate::http::system_time_ms().map_err(|cause| cause.to_string())?;
        let cold_check = crate::pa_watchdog::audit_cold_wake(
            services,
            &descriptor,
            &request.requester,
            &request.id,
            DRAIN_WAKE_REASON,
            now_ms,
        )
        .await
        .map_err(|cause| format!("cold-wake audit failed, nothing sent: {cause}"))?;
        services.delivery.accept(msg).await.map(|mut receipt| {
            receipt.cold_check = Some(cold_check);
            receipt
        })
    };
    match sent {
        Ok(receipt) if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) => {
            Err(format!("refused: {:?}", receipt.outcome))
        }
        Ok(receipt) => Ok(receipt.at),
        Err(cause) => Err(cause.to_string()),
    }
}

/// What the time-limit pass did for one request (plan 205, s12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitAction {
    /// The request id.
    pub request: String,
    /// `check-in` to the holder, or `notice` to the PA (else the prime).
    pub kind: &'static str,
    /// Who it went to, `None` when nobody could be told.
    pub to: Option<SeatId>,
    /// Delivery's outcome word, or the error.
    pub delivery: String,
}

/// The time-limit pass, run inside the existing PA watchdog round (s12):
/// no timer of its own. One indexed read lists the open limited requests;
/// each costs one `LIMIT 1` read of its holder's latest card.
///
/// A request's clock starts at the later of its delivery and its holder's
/// latest card, so every card update restarts it. Past the limit, the holder
/// gets one check-in. After it, a whole further limit with no card update
/// tells the project's PA (else the prime) once. Each step is recorded before
/// it is sent, so a round never sends one twice. Nothing is ever stopped.
pub async fn time_limits(services: &Services, now_ms: u64) -> Vec<LimitAction> {
    let store = services.governance.store().clone();
    let open = match store.open_limited_role_requests().await {
        Ok(open) => open,
        Err(cause) => {
            eprintln!("pij-rs: time limits: open requests unavailable: {cause}");
            return Vec::new();
        }
    };
    let mut actions = Vec::new();
    for request in open {
        let (Some(limit), Some(holder), Some(delivered_at)) = (
            request.limit_ms,
            request.holder.as_deref().map(SeatId::from),
            request.delivered_at,
        ) else {
            continue;
        };
        let card_at = match services
            .spine
            .latest_matching(&holder, &["report.now"])
            .await
        {
            Ok(card) => card.map_or(0, |card| card.at),
            Err(cause) => {
                eprintln!("pij-rs: time limits: card for {holder}: {cause}");
                continue;
            }
        };
        let action = match request.checkin_at {
            None => {
                if now_ms < delivered_at.max(card_at).saturating_add(limit) {
                    continue;
                }
                check_in(services, &store, &request, &holder, limit, now_ms).await
            }
            Some(checked_in) => {
                if now_ms < checked_in.max(card_at).saturating_add(limit) {
                    continue;
                }
                notice(services, &store, &request, &holder, limit, now_ms).await
            }
        };
        if let Some(action) = action {
            actions.push(action);
        }
    }
    actions
}

async fn check_in(
    services: &Services,
    store: &SqliteOrchestration,
    request: &RoleRequest,
    holder: &SeatId,
    limit: u64,
    now_ms: u64,
) -> Option<LimitAction> {
    match store
        .mark_role_request_checked_in(&request.id, now_ms)
        .await
    {
        Ok(true) => {}
        Ok(false) => return None,
        Err(cause) => {
            eprintln!("pij-rs: time limits: check-in for {}: {cause}", request.id);
            return None;
        }
    }
    let address = RoleAddress {
        role: request.role.clone(),
        instance: request.instance.clone(),
    }
    .display();
    let body = format!(
        "[pij time limit] Request {} ({address} in {}, from {}) passed its limit of {} with no card update. \
         Write down what you have, update your card (pij report now \"<what you did>\" \"<what is next>\"), \
         and say why it needs longer. Nothing has been stopped.",
        request.id,
        request.project,
        request.requester,
        human_duration(limit)
    );
    let to = live_seat(services.registry.as_ref(), holder.as_str())
        .await
        .ok()
        .flatten();
    let delivery = match &to {
        Some(seat) => {
            crate::pa_watchdog::wake_pa(
                services,
                seat,
                body,
                format!("role-limit-checkin-{}", request.id),
                "role-limit-check-in",
                now_ms,
            )
            .await
            .0
        }
        None => "holder gone".to_string(),
    };
    Some(LimitAction {
        request: request.id.clone(),
        kind: "check-in",
        to: to.map(|seat| seat.id),
        delivery,
    })
}

async fn notice(
    services: &Services,
    store: &SqliteOrchestration,
    request: &RoleRequest,
    holder: &SeatId,
    limit: u64,
    now_ms: u64,
) -> Option<LimitAction> {
    match store.mark_role_request_escalated(&request.id, now_ms).await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(cause) => {
            eprintln!("pij-rs: time limits: notice for {}: {cause}", request.id);
            return None;
        }
    }
    let target = match store.assignments(&request.project).await {
        Ok(assignments) => wake_target(services, store, &assignments)
            .await
            .ok()
            .flatten(),
        Err(cause) => {
            eprintln!(
                "pij-rs: time limits: assignments for {}: {cause}",
                request.project
            );
            None
        }
    };
    let address = RoleAddress {
        role: request.role.clone(),
        instance: request.instance.clone(),
    }
    .display();
    let body = format!(
        "[pij time limit] {holder} has not answered the check-in for request {} ({address} in {}, from {}): \
         no output and no card update for {} since. Nothing has been stopped. Next: look at it \
         (pij state {holder}), or give the role another holder (pij directory release {} {holder} --why \"<why>\" --project {}).",
        request.id,
        request.project,
        request.requester,
        human_duration(limit),
        address.trim_start_matches(ROLE_PREFIX),
        request.project,
    );
    let delivery = match &target {
        Some(seat) => {
            crate::pa_watchdog::wake_pa(
                services,
                seat,
                body,
                format!("role-limit-notice-{}", request.id),
                "role-limit-notice",
                now_ms,
            )
            .await
            .0
        }
        None => "no live PA or prime to tell".to_string(),
    };
    Some(LimitAction {
        request: request.id.clone(),
        kind: "notice",
        to: target.map(|seat| seat.id),
        delivery,
    })
}
