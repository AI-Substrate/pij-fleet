//! `pij send-file` and `pij file list|accept|decline` (Jordan, 2026-11-28):
//! hand a file of up to 50 MiB to another seat, cross-machine first.
//!
//! Local verbs come to `POST /v1/files` as argv plus caller, like
//! `pij requests`; the daemon runs as the caller's user, so it reads the
//! source and writes the target itself and no bytes cross loopback. To
//! `seat@machine` the sender's daemon stages a snapshot, streams it to the
//! paired daemon's `POST /v1/files/inbound` (the pairing's key, its one
//! connection), and deletes the snapshot once that daemon answers. The bytes
//! live on the recipient's machine, with the recipient's daemon, which tells
//! its seat and later the sender how it ended.
//!
//! Cost: no loop, poll, timer or launch. Expiry runs at daemon start and on
//! each send ([`prune`]); each request is one read of the folder at most.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Extension, Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use pij_core::files::{
    self, E_CORRUPT, E_NO_SUCH_FILE, E_PEER, E_SOURCE, MAX_BYTES, NOTE_MAX_CHARS, Outcome,
};
use pij_core::model::{DeliveryOutcome, Destination, Envelope, ErrorKind, Msg, Receipt, SeatId};
use pij_core::render;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::StreamExt as _;

use super::auth::AuthenticatedMachine;
use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, PeerEndpoint, SendRequest, cold_wake, envelope, system_time_ms};
use crate::Services;
use crate::community::Refusal;
use crate::delivery::role_send::RoleAddress;
use crate::federation::{FederationService, peer_reason};
use crate::transfers::{Pending, Transfers};

/// The peer route a sender's daemon streams a file to.
pub(crate) const INBOUND_PATH: &str = "/v1/files/inbound";
/// The header carrying a streamed file's description, hex-encoded JSON.
const META_HEADER: &str = "x-pij-file";
/// A whole 50 MiB upload, on a slow link.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const SEND_USAGE: &str =
    "pij send-file <seat|role:<id>|seat@machine> <path> [--note <text>] [--force --reason <text>]";
const ACCEPT_USAGE: &str = "pij file accept <code> --to <path> [--force]";
const DECLINE_USAGE: &str = "pij file decline <code>";
const LIST_USAGE: &str = "pij file list";

/// What a sender's daemon tells the recipient's about a streamed file.
#[derive(Debug, Serialize, Deserialize)]
struct InboundMeta {
    from: String,
    to: String,
    name: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    reason: Option<String>,
}

/// A refusal, or a response another check already built (a cold wake).
enum Failed {
    Refusal(Refusal),
    Response(Box<Response>),
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Self {
        Self::Refusal(refusal)
    }
}

impl Failed {
    fn respond(self, command: &str) -> Response {
        match self {
            Self::Refusal(refusal) => refused(command, &refusal),
            Self::Response(response) => *response,
        }
    }
}

/// `POST /v1/files`: argv is `send-file ...` or `file <leaf> ...`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let argv = &request.argv;
    let (command, leaf, args) = match (argv.first().map(String::as_str), argv.get(1)) {
        (Some("send-file"), _) => ("pij send-file", "send", argv.get(1..).unwrap_or_default()),
        (Some("file"), Some(leaf)) if matches!(leaf.as_str(), "list" | "accept" | "decline") => {
            let command = match leaf.as_str() {
                "list" => "pij file list",
                "accept" => "pij file accept",
                _ => "pij file decline",
            };
            (command, leaf.as_str(), argv.get(2..).unwrap_or_default())
        }
        _ => {
            if pij_core::directory::wants_help(argv) {
                return envelope(
                    StatusCode::OK,
                    &Envelope::ok("pij file", json!({"help": files::HELP})),
                );
            }
            return refused(
                "pij file",
                &Refusal::new(
                    "E-RS-ARG",
                    format!(
                        "usage: {SEND_USAGE} | {LIST_USAGE} | {ACCEPT_USAGE} | {DECLINE_USAGE}"
                    ),
                ),
            );
        }
    };
    if pij_core::directory::wants_help(args) {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(command, json!({"help": files::HELP})),
        );
    }
    let actor = match resolve_seat(
        &state,
        command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat.id,
        Resolved::Refusal(response) => return response,
    };
    let result = match leaf {
        "send" => send(&state, &actor, &request.caller, args).await,
        "list" => list(&state, &actor, args).map_err(Failed::from),
        "accept" => accept(&state, &actor, &request.caller, args).await,
        _ => decline(&state, &actor, args).map_err(Failed::from),
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(failed) => failed.respond(command),
    }
}

/// Positions, `--name value` pairs, and bare switches.
struct Parsed {
    positions: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Parsed {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn switch(&self, name: &str) -> bool {
        self.switches.iter().any(|key| key == name)
    }
}

fn parse(
    tokens: &[String],
    values: &[&str],
    switches: &[&str],
    usage: &str,
) -> Result<Parsed, Refusal> {
    let mut parsed = Parsed {
        positions: Vec::new(),
        values: Vec::new(),
        switches: Vec::new(),
    };
    let mut tokens = tokens.iter();
    while let Some(token) = tokens.next() {
        if token == "--" {
            parsed.positions.extend(tokens.by_ref().cloned());
            break;
        }
        let Some(flag) = token.strip_prefix("--") else {
            parsed.positions.push(token.clone());
            continue;
        };
        if flag == "json" {
            continue;
        }
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (flag, None),
        };
        if switches.contains(&name) && inline.is_none() {
            parsed.switches.push(name.to_string());
        } else if values.contains(&name) {
            let value = match inline {
                Some(value) => value,
                None => tokens.next().cloned().ok_or_else(|| {
                    Refusal::new("E-RS-ARG", format!("--{name} requires a value"))
                })?,
            };
            parsed.values.push((name.to_string(), value));
        } else {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("unsupported --{name}; usage: {usage}"),
            ));
        }
    }
    Ok(parsed)
}

/// `path` as the caller meant it: absolute, or under the caller's folder.
fn absolute(caller: &CallerContext, path: &str) -> Result<PathBuf, Refusal> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match caller
        .cwd
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_absolute())
    {
        Some(cwd) => Ok(Path::new(cwd).join(path)),
        None => Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "{} is relative and the caller's folder is unknown; give an absolute path",
                path.display()
            ),
        )),
    }
}

fn now() -> Result<u64, Refusal> {
    system_time_ms().map_err(|error| Refusal::new(E_SOURCE, error.to_string()))
}

fn store_refusal(error: impl std::fmt::Display) -> Refusal {
    Refusal::new("E-RS-STORE", error.to_string())
}

async fn live_seat(services: &Services, seat: &SeatId) -> Result<bool, Refusal> {
    Ok(services
        .registry
        .get(seat)
        .await
        .map_err(store_refusal)?
        .is_some_and(|row| row.tombstoned_at.is_none()))
}

async fn send(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    args: &[String],
) -> Result<Value, Failed> {
    let parsed = parse(args, &["note", "reason", "project"], &["force"], SEND_USAGE)?;
    let [to, path] = parsed.positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {SEND_USAGE}")).into());
    };
    let note = parsed.value("note").map(str::to_string);
    if note
        .as_deref()
        .is_some_and(|note| note.chars().count() > NOTE_MAX_CHARS)
    {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("--note is longer than {NOTE_MAX_CHARS} characters"),
        )
        .into());
    }
    let source = absolute(caller, path)?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| files::is_file_name(name))
        .ok_or_else(|| {
            Refusal::new(
                E_SOURCE,
                format!(
                    "{} does not end in a file name pij can carry",
                    source.display()
                ),
            )
        })?
        .to_string();
    let destination = destination(state, caller, to, parsed.value("project")).await?;
    if destination.machine.is_none() && destination.seat == *actor {
        return Err(Refusal::new("E-RS-ARG", "a seat cannot send a file to itself").into());
    }
    prune(&state.services, state.federation.as_ref(), now()?).await;
    let file = Outbound {
        from: actor.clone(),
        to: destination.seat,
        source,
        name,
        note,
        force: parsed.switch("force"),
        reason: parsed.value("reason").map(str::to_string),
    };
    match destination.machine {
        None => send_local(state, file).await,
        Some(alias) => send_remote(state, &alias, file).await,
    }
}

/// One file a seat on this machine is sending.
struct Outbound {
    from: SeatId,
    to: SeatId,
    source: PathBuf,
    name: String,
    note: Option<String>,
    force: bool,
    reason: Option<String>,
}

/// `seat`, `seat@machine` (this machine's alias means here), or a role with
/// exactly one live holder on this machine.
async fn destination(
    state: &AppState,
    caller: &CallerContext,
    to: &str,
    project: Option<&str>,
) -> Result<Destination, Refusal> {
    if let Some(address) = RoleAddress::parse(to) {
        let address = address?;
        return Ok(Destination::local(
            role_holder(state, caller, &address, project).await?,
        ));
    }
    if project.is_some() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--project names the directory for a send to role:<id>; a send to a seat has none",
        ));
    }
    let mut destination = pij_core::address::parse_destination(to)
        .map_err(|error| Refusal::new("E-RS-ARG", error.to_string()))?;
    if destination.machine.as_deref() == Some(state.machine_alias.as_str()) {
        destination.machine = None;
    }
    Ok(destination)
}

/// A role's one live holder. A file is never queued for a role: with none,
/// or several, the sender picks a seat.
async fn role_holder(
    state: &AppState,
    caller: &CallerContext,
    address: &RoleAddress,
    project: Option<&str>,
) -> Result<SeatId, Refusal> {
    let (project, _) =
        super::directory::project_and_repo(state, caller, project.map(str::to_string)).await?;
    let rows = state
        .services
        .governance
        .store()
        .assignments(&project)
        .await
        .map_err(store_refusal)?;
    let mut live: Vec<SeatId> = Vec::new();
    for row in rows.iter().filter(|row| {
        row.to.is_none()
            && row.role == address.role
            && (address.instance.is_empty() || row.instance == address.instance)
    }) {
        let seat = SeatId::from(row.seat.as_str());
        if !live.contains(&seat) && live_seat(&state.services, &seat).await? {
            live.push(seat);
        }
    }
    match live.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(Refusal::new(
            "E-RS-NOT-FOUND",
            format!(
                "{} has no live holder in project {}; a file is never queued for a role, so send it to a seat",
                address.display(),
                render::project(&project)
            ),
        )),
        several => Err(Refusal::new(
            "E-RS-ROLE-HOLDERS",
            format!(
                "{} has {} live holders ({}); send the file to one of them",
                address.display(),
                several.len(),
                several
                    .iter()
                    .map(|seat| render::seat(seat.as_str()).into_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

async fn send_local(state: &AppState, file: Outbound) -> Result<Value, Failed> {
    let services = &state.services;
    if !live_seat(services, &file.to).await? {
        return Err(Refusal::new(
            "E-RS-NOT-FOUND",
            format!(
                "no live seat {} on this machine",
                render::seat(file.to.as_str())
            ),
        )
        .into());
    }
    let code = Transfers::mint_code()?;
    cold_wake::guard(
        state,
        "pij send-file",
        &file.from,
        &file.to,
        &notice_id(&code),
        cold_wake::Override {
            force: file.force,
            reason: file.reason.as_deref(),
            from_machine: None,
        },
    )
    .await
    .map_err(Failed::Response)?;
    let transfers = &services.transfers;
    transfers.prepare()?;
    let _held = transfers.hold(&code);
    let (size, sha256) = transfers.ingest_path(&code, &file.source).await?;
    let pending = Pending {
        code: code.clone(),
        from: file.from.to_string(),
        from_machine: None,
        to: file.to.to_string(),
        name: file.name,
        size,
        sha256,
        note: file.note,
        created_at: now()?,
    };
    store_and_notify(services, pending)
        .await
        .map_err(Failed::from)
}

/// Record `pending` (bytes already in place), then send its notice; on a
/// notice that does not go through, delete it again.
async fn store_and_notify(services: &Services, pending: Pending) -> Result<Value, Refusal> {
    let transfers = &services.transfers;
    if let Err(refusal) = transfers.record(&pending) {
        transfers.remove(&pending.code);
        return Err(refusal);
    }
    let receipt = notify_recipient(services, &pending).await;
    let receipt = match receipt {
        Ok(receipt) if !matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) => receipt,
        Ok(receipt) => {
            transfers.remove(&pending.code);
            return Err(Refusal::new(
                E_PEER,
                format!(
                    "the notice to {} was refused ({:?}); the file was deleted",
                    render::seat(&pending.to),
                    receipt.outcome
                ),
            ));
        }
        Err(error) => {
            transfers.remove(&pending.code);
            return Err(Refusal::new(
                E_PEER,
                format!(
                    "the notice to {} failed: {error}; the file was deleted",
                    render::seat(&pending.to)
                ),
            ));
        }
    };
    Ok(json!({
        "code": pending.code,
        "to": pending.to,
        "name": pending.name,
        "size": pending.size,
        "sha256": pending.sha256,
        "receipt": receipt,
    }))
}

fn notice_id(code: &str) -> String {
    format!("file-{code}")
}

async fn notify_recipient(
    services: &Services,
    pending: &Pending,
) -> pij_core::error::Result<Receipt> {
    let seat = render::seat(&pending.from);
    let sender = match &pending.from_machine {
        Some(machine) => format!("{seat}@{machine}"),
        None => seat.into_owned(),
    };
    let msg = Msg {
        from: SeatId::from(pending.from.as_str()),
        from_machine: pending.from_machine.clone(),
        to: SeatId::from(pending.to.as_str()),
        body: files::notice_body(&files::Notice {
            from: &sender,
            name: &pending.name,
            size: pending.size,
            sha256: &pending.sha256,
            note: pending.note.as_deref(),
            code: &pending.code,
        }),
        msg_id: notice_id(&pending.code),
        in_reply_to: None,
        command: None,
    };
    if pending.from_machine.is_some() {
        services.delivery.accept_forwarded(msg).await
    } else {
        services.delivery.accept(msg).await
    }
}

async fn send_remote(state: &AppState, alias: &str, file: Outbound) -> Result<Value, Failed> {
    let Some(federation) = state.federation.as_ref() else {
        return Err(Refusal::new(
            E_PEER,
            format!("no machine is paired with this one, so {alias} cannot be reached; see docs/how/pij-federation.md"),
        )
        .into());
    };
    let Some(endpoint) = federation.endpoint(alias).cloned() else {
        return Err(Refusal::new(
            E_PEER,
            format!("machine {alias} is not paired with this one (peers.toml)"),
        )
        .into());
    };
    let transfers = &state.services.transfers;
    transfers.prepare()?;
    let code = Transfers::mint_code()?;
    let _held = transfers.hold(&code);
    // A snapshot, so the sha256 sent ahead is the bytes that follow.
    let staged = transfers.ingest_path(&code, &file.source).await;
    let result = match staged {
        Ok((size, sha256)) => {
            let meta = InboundMeta {
                from: file.from.to_string(),
                to: file.to.to_string(),
                name: file.name,
                size,
                sha256,
                note: file.note,
                force: file.force,
                reason: file.reason,
            };
            upload(&endpoint, alias, &transfers.root().join(&code), &meta).await
        }
        Err(refusal) => Err(refusal.into()),
    };
    // Sent or not, this machine's copy goes: the peer holds the file now, or
    // nobody does and the sender sends it again.
    transfers.remove(&code);
    result
}

async fn upload(
    endpoint: &PeerEndpoint,
    alias: &str,
    staged: &Path,
    meta: &InboundMeta,
) -> Result<Value, Failed> {
    let failed = |what: String| Refusal::new(E_PEER, what);
    // `no_proxy`, as every federation client: the request carries the
    // pairing's key.
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|error| failed(format!("could not build the peer client: {error}")))?;
    let header =
        files::hex(&serde_json::to_vec(meta).map_err(|error| {
            failed(format!("could not encode the file's description: {error}"))
        })?);
    let body = tokio::fs::File::open(staged)
        .await
        .map_err(|error| failed(format!("the staged copy is unreadable: {error}")))?;
    let url = format!("{}{INBOUND_PATH}", endpoint.base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(&endpoint.bearer_key)
        .header(META_HEADER, header)
        .body(reqwest::Body::from(body))
        .send()
        .await
        .map_err(|error| {
            let what = if error.is_connect() {
                format!("could not reach {alias}: {error}; nothing was sent")
            } else {
                format!("the upload to {alias} did not finish: {error}; it may not have been stored there, send it again")
            };
            failed(what)
        })?;
    let text = response
        .text()
        .await
        .map_err(|error| failed(format!("{alias} gave no complete answer: {error}")))?;
    let answer: Envelope<Value> = serde_json::from_str(&text)
        .map_err(|_| failed(format!("{alias} answered something this build cannot read")))?;
    if answer.ok {
        let mut data = answer.data.unwrap_or(Value::Null);
        data["to"] = json!(format!("{}@{alias}", meta.to));
        data["machine"] = json!(alias);
        return Ok(data);
    }
    let theirs = answer.meta.as_deref().unwrap_or("no reason given");
    let mut refusal = failed(format!(
        "{alias} refused the file: {}; nothing is stored on either machine",
        peer_reason(theirs)
    ));
    if let Some(next) = answer
        .details
        .as_ref()
        .and_then(|details| details["next"].as_str())
    {
        refusal = refusal.with_next(peer_reason(next));
    }
    Err(refusal.into())
}

/// `POST /v1/files/inbound`: a paired machine's daemon streams one file to a
/// seat here. Only a peer key reaches this (plan 164's scope, widened by one
/// route); the sender seat is the peer's to name, as on `/v1/send`, and the
/// machine is stamped from the key.
pub(crate) async fn inbound(
    State(state): State<AppState>,
    Extension(caller): Extension<AuthenticatedMachine>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let command = "pij send-file";
    let AuthenticatedMachine::Peer(alias) = caller else {
        return refused(
            command,
            &Refusal::new(
                "E-RS-ARG",
                "this route takes a file from a paired machine's daemon; on this machine run pij send-file",
            ),
        );
    };
    match inbound_file(&state, &alias, &headers, body).await {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(failed) => failed.respond(command),
    }
}

/// A seat id a peer may name: one plain token. Shown only through
/// [`render::seat`], so a non-canonical one can carry no framing.
fn plain_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && !id
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '@' | '/'))
}

fn read_meta(headers: &HeaderMap) -> Result<InboundMeta, Refusal> {
    let bad = || {
        Refusal::new(
            "E-RS-ARG",
            format!("the {META_HEADER} header is missing or unreadable"),
        )
    };
    let text = headers
        .get(META_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(bad)?;
    if text.len() % 2 != 0 {
        return Err(bad());
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| bad())?;
    let meta: InboundMeta = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    let fits = plain_id(&meta.from)
        && plain_id(&meta.to)
        && files::is_file_name(&meta.name)
        && files::is_sha256(&meta.sha256)
        && meta
            .note
            .as_deref()
            .is_none_or(|note| note.chars().count() <= NOTE_MAX_CHARS);
    if !fits {
        return Err(Refusal::new(
            "E-RS-ARG",
            "the file's description does not fit pij's grammar (seat ids, file name, sha256, note)",
        ));
    }
    if meta.size > MAX_BYTES {
        return Err(Refusal::new(
            files::E_TOO_LARGE,
            format!(
                "the file is {} ({} bytes); the limit is {} ({MAX_BYTES} bytes)",
                files::human_size(meta.size),
                meta.size,
                files::human_size(MAX_BYTES)
            ),
        ));
    }
    Ok(meta)
}

async fn inbound_file(
    state: &AppState,
    alias: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Value, Failed> {
    let meta = read_meta(headers)?;
    let services = &state.services;
    let (from, to) = (
        SeatId::from(meta.from.as_str()),
        SeatId::from(meta.to.as_str()),
    );
    if !live_seat(services, &to).await? {
        return Err(Refusal::new(
            "E-RS-NOT-FOUND",
            format!("no live seat {} on this machine", render::seat(&meta.to)),
        )
        .into());
    }
    let code = Transfers::mint_code()?;
    // Before a byte is read: a cold recipient's refusal costs no upload.
    cold_wake::guard(
        state,
        "pij send-file",
        &from,
        &to,
        &notice_id(&code),
        cold_wake::Override {
            force: meta.force,
            reason: meta.reason.as_deref(),
            from_machine: Some(alias),
        },
    )
    .await
    .map_err(Failed::Response)?;
    prune(services, state.federation.as_ref(), now()?).await;
    let transfers = &services.transfers;
    transfers.prepare()?;
    transfers.check_floor(meta.size)?;
    let _held = transfers.hold(&code);
    let mut incoming = transfers.incoming(&code, meta.size).await?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| Refusal::new(E_PEER, format!("the upload broke off: {error}")))?;
        incoming.write(&chunk).await?;
    }
    let (size, sha256) = incoming.finish().await?;
    if size != meta.size || sha256 != meta.sha256 {
        transfers.remove(&code);
        return Err(Refusal::new(
            E_CORRUPT,
            format!(
                "received {size} bytes hashing to {sha256}; the sender described {} bytes hashing to {}; nothing was kept",
                meta.size, meta.sha256
            ),
        )
        .into());
    }
    let pending = Pending {
        code,
        from: meta.from,
        from_machine: Some(alias.to_string()),
        to: meta.to,
        name: meta.name,
        size,
        sha256,
        note: meta.note,
        created_at: now()?,
    };
    store_and_notify(services, pending)
        .await
        .map_err(Failed::from)
}

fn list(state: &AppState, actor: &SeatId, args: &[String]) -> Result<Value, Refusal> {
    let parsed = parse(args, &[], &[], LIST_USAGE)?;
    if !parsed.positions.is_empty() {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {LIST_USAGE}")));
    }
    let at = now()?;
    let files: Vec<Value> = state
        .services
        .transfers
        .list()
        .into_iter()
        .filter(|pending| pending.to == actor.as_str())
        .map(|pending| {
            json!({
                "code": pending.code,
                "from": pending.sender(),
                "name": pending.name,
                "size": pending.size,
                "sha256": pending.sha256,
                "note": pending.note,
                "created_at": pending.created_at,
                "age_ms": at.saturating_sub(pending.created_at),
            })
        })
        .collect();
    Ok(json!({"seat": actor, "files": files}))
}

/// The pending file `code`, if it was sent to `actor`, held for this request.
/// An unknown code and someone else's file read the same, so a code is
/// never confirmed to a seat it was not sent to.
fn claim(
    state: &AppState,
    actor: &SeatId,
    code: &str,
) -> Result<(Pending, crate::transfers::Held), Refusal> {
    let transfers = &state.services.transfers;
    let unknown = || {
        Refusal::new(
            E_NO_SUCH_FILE,
            format!(
                "no file with code {} is waiting for {}",
                render::msg_id(code),
                render::seat(actor.as_str())
            ),
        )
        .with_next("pij file list")
    };
    let mine = |pending: &Pending| pending.to == actor.as_str();
    if !files::is_code(code) || !transfers.get(code).is_some_and(|pending| mine(&pending)) {
        return Err(unknown());
    }
    let held = transfers.hold(code).ok_or_else(|| {
        Refusal::new(
            "E-RS-FILE-BUSY",
            "another accept or decline of this file is running",
        )
    })?;
    let pending = transfers.get(code).filter(mine).ok_or_else(unknown)?;
    Ok((pending, held))
}

async fn accept(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    args: &[String],
) -> Result<Value, Failed> {
    let parsed = parse(args, &["to"], &["force"], ACCEPT_USAGE)?;
    let ([code], Some(to)) = (parsed.positions.as_slice(), parsed.value("to")) else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {ACCEPT_USAGE}")).into());
    };
    let target = absolute(caller, to)?;
    let (pending, _held) = claim(state, actor, code)?;
    state
        .services
        .transfers
        .save(&pending, &target, parsed.switch("force"))
        .await?;
    state.services.transfers.remove(&pending.code);
    tell_outcome(
        &state.services,
        state.federation.as_ref(),
        &pending,
        Outcome::Accepted,
    );
    Ok(json!({
        "code": pending.code,
        "from": pending.sender(),
        "name": pending.name,
        "size": pending.size,
        "sha256": pending.sha256,
        "saved_to": target,
    }))
}

fn decline(state: &AppState, actor: &SeatId, args: &[String]) -> Result<Value, Refusal> {
    let parsed = parse(args, &[], &[], DECLINE_USAGE)?;
    let [code] = parsed.positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {DECLINE_USAGE}")));
    };
    let (pending, _held) = claim(state, actor, code)?;
    state.services.transfers.remove(&pending.code);
    tell_outcome(
        &state.services,
        state.federation.as_ref(),
        &pending,
        Outcome::Declined,
    );
    Ok(json!({
        "code": pending.code,
        "from": pending.sender(),
        "name": pending.name,
        "declined": true,
    }))
}

/// Delete every file unclaimed for 24 h, telling its sender and recipient.
/// Runs at daemon start and on each send; never on a timer.
pub async fn prune(services: &Services, federation: Option<&Arc<FederationService>>, now_ms: u64) {
    for pending in services.transfers.take_expired(now_ms) {
        tell_outcome(services, federation, &pending, Outcome::Expired);
    }
}

/// The sender's FYI (and, on expiry, the recipient's), from a task of its
/// own so the caller never waits on a peer.
fn tell_outcome(
    services: &Services,
    federation: Option<&Arc<FederationService>>,
    pending: &Pending,
    outcome: Outcome,
) {
    let (services, federation, pending) = (services.clone(), federation.cloned(), pending.clone());
    tokio::spawn(async move {
        let msg_id = format!("file-{}-{}", pending.code, outcome.as_str());
        if outcome == Outcome::Expired {
            let _ = services
                .delivery
                .hold_fyi(Msg {
                    from: SeatId::from(pij_core::BG_ACTOR),
                    from_machine: None,
                    to: SeatId::from(pending.to.as_str()),
                    body: files::outcome_body(outcome, &pending.name, &pending.code, "you"),
                    msg_id: msg_id.clone(),
                    in_reply_to: None,
                    command: None,
                })
                .await;
        }
        match (&pending.from_machine, &federation) {
            (None, _) => {
                let _ = services
                    .delivery
                    .hold_fyi(Msg {
                        from: SeatId::from(pij_core::BG_ACTOR),
                        from_machine: None,
                        to: SeatId::from(pending.from.as_str()),
                        body: files::outcome_body(
                            outcome,
                            &pending.name,
                            &pending.code,
                            &pending.to,
                        ),
                        msg_id,
                        in_reply_to: None,
                        command: None,
                    })
                    .await;
            }
            (Some(machine), Some(federation)) => {
                let to = format!("{}@{}", pending.to, federation.local_alias());
                let request = SendRequest {
                    // From the recipient, the seat it concerns: a peer
                    // refuses `pij-bg` as a sender.
                    from: SeatId::from(pending.to.as_str()),
                    to: Destination {
                        seat: SeatId::from(pending.from.as_str()),
                        machine: Some(machine.clone()),
                    },
                    body: files::outcome_body(outcome, &pending.name, &pending.code, &to),
                    msg_id,
                    from_machine: None,
                    in_reply_to: None,
                    fyi: true,
                    force: false,
                    reason: None,
                };
                if let Err(error) = federation.enqueue_remote(&request).await {
                    eprintln!(
                        "pij-rs: could not queue the file outcome for {}@{machine}: {error}",
                        pending.from
                    );
                }
            }
            (Some(machine), None) => eprintln!(
                "pij-rs: file {} came from {machine}, which is no longer paired; its sender was not told",
                pending.code
            ),
        }
    });
}

fn refused(command: &str, refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-ARG" => StatusCode::BAD_REQUEST,
        "E-RS-NOT-FOUND" | E_NO_SUCH_FILE => StatusCode::NOT_FOUND,
        files::E_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
        "E-RS-STORE" => StatusCode::INTERNAL_SERVER_ERROR,
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
mod tests;
