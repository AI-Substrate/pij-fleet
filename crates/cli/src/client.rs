//! Authenticated HTTP client for the daemon wire.

use std::collections::BTreeMap;
use std::path::Path;

use pij_core::error::{PijError, Result};
use pij_core::model::{
    Destination, ENVELOPE_VERSION, Envelope, ErrorKind, Harness, Msg, Receipt, SeatDescriptor,
    SeatId,
};
use pij_daemon::delivery::InboxClaim;
use pij_daemon::http::InboxAckRequest;
pub use pij_daemon::http::{
    CallerContext, FederatedRoster, IdentityRequest, Phonehome, ReviveRequest, SpawnRequest,
    StateCard, StreamFrame,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A registration claim. The daemon verifies it before storing anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Registration {
    /// Existing same-process seat replaced at a native session boundary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<SeatId>,
    /// Requested seat id.
    pub id: SeatId,
    /// Host harness.
    pub harness: Harness,
    /// Absolute working folder.
    pub folder: String,
    /// Loaded pij extension build, when the registering runtime reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_build: Option<String>,
    /// Real directory the runtime loaded its pij extension from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_path: Option<String>,
    /// Tmux pane, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// Process id, present only with `proc_start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Process start stamp, present only with `pid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<u64>,
    /// Spawn correlation id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_id: Option<String>,
    /// Requested model selector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Model provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Requested reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Governing seat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<SeatId>,
    /// Explicit role assertion; omission preserves the daemon's assignment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Whether delivery uses a relay.
    pub relay: bool,
}

impl Registration {
    /// Reject a half process identity before sending a claim the daemon cannot
    /// corroborate.
    pub fn validate(&self) -> std::result::Result<(), &'static str> {
        if self
            .role
            .as_deref()
            .is_some_and(|role| role.trim().is_empty())
        {
            return Err("--role must be a nonempty string");
        }
        if self.pid.is_some() == self.proc_start.is_some() {
            Ok(())
        } else {
            Err("--pid and --proc-start must be supplied together")
        }
    }
}

/// One durable-send request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SendRequest {
    /// Sending seat.
    pub from: SeatId,
    /// Parsed local or remote destination.
    pub to: Destination,
    /// Literal message body.
    pub body: String,
    /// Caller-supplied correlation id.
    pub msg_id: String,
    /// Message id being answered.
    pub in_reply_to: Option<String>,
    /// A control command, mutually exclusive with a non-empty body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Hold for the recipient's next real turn instead of delivering (plan 158).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fyi: bool,
    /// Wake a cold recipient anyway (plan 157 phase 2); needs `reason`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
    /// Why a forced cold wake is worth it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// StreamFrame is owned by the daemon wire module and re-exported here so the
// writer and reader cannot drift into two independently valid JSON shapes.

/// An authenticated daemon client.
///
/// # Composition recipe
///
/// `use pij_cli::DaemonClient;` then construct once with
/// `DaemonClient::new(&state_dir, &addr)`. The client reads
/// `<state-dir>/daemon.key`; no daemon `AdapterChoice` arm changes for this
/// composition root.
#[derive(Clone, Debug)]
pub struct DaemonClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl DaemonClient {
    /// Read the daemon's boot key and construct a client.
    ///
    /// # Errors
    /// When the key cannot be read.
    pub fn new(state_dir: &Path, addr: &str) -> Result<Self> {
        let token = pij_daemon::auth::read_key(&state_dir.join("daemon.key"))?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url: format!("http://{addr}"),
            token,
        })
    }

    /// Ask whether the daemon is healthy.
    pub async fn ping(&self) -> Envelope<Value> {
        self.get("pij ping", "/health").await
    }

    /// Submit one verified registration claim.
    pub async fn register(&self, registration: &Registration) -> Envelope<SeatDescriptor> {
        self.post("pij register", "/v1/register", registration)
            .await
    }

    /// Adopt this seat: the DAEMON mints the name and derives the identity.
    ///
    /// The body carries the argv this CLI would have typed, so the `pij-rs`
    /// caller and the generation shim reach the daemon's parser by the same
    /// path. Two parsers for one command line is how the two surfaces come to
    /// disagree about what `--parent` means.
    pub async fn adopt(&self, request: &IdentityRequest) -> Envelope<SeatDescriptor> {
        self.post("pij adopt", "/v1/adopt", request).await
    }

    /// Ask the daemon which seat this caller is.
    pub async fn whoami(&self, request: &IdentityRequest) -> Envelope<SeatDescriptor> {
        self.post("pij whoami", "/v1/whoami", request).await
    }

    /// Confirm the binding `adopt` already made.
    pub async fn phonehome(&self, request: &IdentityRequest) -> Envelope<Phonehome> {
        self.post("pij phonehome", "/v1/phonehome", request).await
    }

    /// Ask the daemon to launch and record one pre-bind seat.
    pub async fn spawn(&self, request: &SpawnRequest) -> Envelope<SeatDescriptor> {
        self.post("pij spawn", "/v1/spawn", request).await
    }

    /// Relaunch one tombstoned seat through the daemon's revive transition.
    pub async fn revive(&self, request: &ReviveRequest) -> Envelope<SeatDescriptor> {
        self.post("pij revive", "/v1/revive", request).await
    }

    /// Durably enqueue one message.
    pub async fn send(&self, request: &SendRequest) -> Envelope<Receipt> {
        self.post("pij send", "/v1/send", request).await
    }

    /// Publish a seat's own busy/idle observation (plan 158; Claude hooks, plan 157).
    pub async fn activity(&self, request: &serde_json::Value) -> Envelope<serde_json::Value> {
        self.post("pij activity", "/v1/activity", request).await
    }

    /// Claim the FYIs held for a seat (plan 158).
    pub async fn fyi_claim(&self, request: &serde_json::Value) -> Envelope<serde_json::Value> {
        self.post("pij fyi-claim", "/v1/fyi/claim", request).await
    }

    /// Read one delivered FYI batch in full (plan 159).
    pub async fn fyi_read(&self, request: &serde_json::Value) -> Envelope<serde_json::Value> {
        self.post("pij fyi-read", "/v1/fyi/read", request).await
    }

    /// Submit a control with caller evidence the daemon must independently verify.
    pub async fn send_control(
        &self,
        request: &SendRequest,
        caller: &CallerContext,
    ) -> Envelope<Receipt> {
        #[derive(Serialize)]
        struct ControlSend<'a> {
            #[serde(flatten)]
            message: &'a SendRequest,
            caller: &'a CallerContext,
        }
        self.post(
            "pij send",
            "/v1/send",
            &ControlSend {
                message: request,
                caller,
            },
        )
        .await
    }

    /// Enqueue one sidecar job (telegram / bg / chore).
    ///
    /// The sidecar consumers are queue-backed, so this is the SHIPPED producer:
    /// live-fire proof that enqueues by hand-written SQL proves the harness, not
    /// the product.
    pub async fn sidecar(&self, request: &Value) -> Envelope<Value> {
        self.post("pij sidecar", "/v1/sidecar", request).await
    }

    /// Forward the background-job grammar and caller evidence to the daemon.
    /// The daemon derives ownership; this request never asserts an owner.
    pub async fn bg(&self, argv: &[String], caller: &CallerContext) -> Envelope<Value> {
        self.post(
            "pij bg",
            "/v1/bg",
            &serde_json::json!({ "argv": argv, "caller": caller }),
        )
        .await
    }

    /// Report this seat's card or declared state.
    ///
    /// The CLI FORWARDS `argv` rather than parsing it: the daemon owns the
    /// grammar, so there is exactly one implementation of the argument shapes
    /// and the two surfaces cannot drift into disagreeing about what
    /// `pij report now` means.
    ///
    /// Forward caller evidence and any explicit agreement claim to the daemon.
    /// Resolving first would discard structured refusal evidence at the CLI edge.
    pub async fn report(
        &self,
        seat: Option<&SeatId>,
        argv: &[String],
        caller: &CallerContext,
    ) -> Envelope<Value> {
        self.post(
            "pij report",
            "/v1/report",
            &serde_json::json!({ "seat": seat, "argv": argv, "caller": caller }),
        )
        .await
    }

    /// List the local authority plus typed remote availability.
    pub async fn list(&self) -> Envelope<FederatedRoster> {
        self.get("pij list", "/v1/seats").await
    }

    /// [`Self::list`] with each local seat's size and coldness (plan 160).
    pub async fn list_sized(&self) -> Envelope<FederatedRoster> {
        self.get("pij list", "/v1/seats?sizes=true").await
    }

    /// Forward list scope arguments to the daemon's shared boolean/cwd parser,
    /// with each local seat's size and coldness (plan 160).
    pub async fn list_scoped(
        &self,
        args: &[String],
        caller: &CallerContext,
    ) -> Envelope<FederatedRoster> {
        let argv: Vec<&str> = std::iter::once("list")
            .chain(args.iter().map(String::as_str))
            .collect();
        self.post(
            "pij list",
            "/v1/seats?sizes=true",
            &serde_json::json!({"argv":argv,"caller":caller}),
        )
        .await
    }

    /// List only this daemon's authoritative registry, excluding retained remote seats.
    pub async fn local_seats(&self) -> Envelope<FederatedRoster> {
        self.get("pij list", "/v1/seats?scope=local").await
    }

    /// Read one seat's card back — `pij state <id>` (plan 114, u-readback).
    ///
    /// POSTs the NATIVE body shape, `{ "id": "<seat>" }`. The route is POST even
    /// though it reads nothing, because the TS routing shim forwards the
    /// operator's argv only on its POST branch: a GET row would arrive with the
    /// `<id>` dropped at the seam
    /// (`.pi/extensions/pij/adapters/generation-router.ts:199-207`). The handler
    /// accepts both shapes so neither caller has to speak the other's.
    pub async fn state(&self, seat: &SeatId) -> Envelope<StateCard> {
        self.post("pij state", "/v1/state", &serde_json::json!({ "id": seat }))
            .await
    }

    /// Read at most one inbox message and acknowledge only after decoding.
    pub async fn inbox(
        &self,
        seat: &SeatId,
        wait: bool,
        caller: &CallerContext,
    ) -> Envelope<Vec<Msg>> {
        let native_cli = caller.copilot_session.is_some();
        let claimed: Envelope<Vec<InboxClaim>> = if native_cli {
            self.post(
                "pij inbox",
                "/v1/shim/inbox",
                &serde_json::json!({
                    "argv": if wait { vec!["inbox", "--wait"] } else { vec!["inbox"] },
                    "caller": caller,
                }),
            )
            .await
        } else {
            self.execute(
                "pij inbox",
                self.http
                    .get(format!("{}/v1/inbox", self.base_url))
                    .query(&[
                        ("seat", seat.as_str()),
                        ("wait", if wait { "true" } else { "false" }),
                    ]),
            )
            .await
        };
        if !claimed.ok {
            return retype_envelope(claimed);
        }
        let Some(claims) = claimed.data else {
            return refusal(
                "pij inbox",
                ErrorKind::Adapter,
                "daemon returned a successful inbox envelope without a claim",
            );
        };
        if claims.len() > 1 {
            return refusal(
                "pij inbox",
                ErrorKind::Adapter,
                "daemon violated inbox serialization by returning more than one claim",
            );
        }
        let messages: Vec<Msg> = claims.iter().map(|claim| claim.message.clone()).collect();
        let mut result = Envelope::ok("pij inbox", messages);
        result.meta = claimed.meta;
        result.details = claimed.details;
        let Some(claim) = claims.first() else {
            return result;
        };
        let acked: Envelope<serde_json::Value> = if native_cli {
            self.post(
                "pij inbox",
                "/v1/shim/inbox/ack",
                &serde_json::json!({
                    "job_id": claim.job_id, "caller": caller,
                }),
            )
            .await
        } else {
            self.post(
                "pij inbox",
                "/v1/inbox/ack",
                &InboxAckRequest {
                    delivery_outcome: None,
                    seat: seat.clone(),
                    job_id: claim.job_id,
                    native: Default::default(),
                    control_outcome: None,
                },
            )
            .await
        };
        if !acked.ok {
            let warning = format!(
                "messages were received but acknowledgement failed; they may be read again: {}",
                acked.meta.as_deref().unwrap_or("no reason given")
            );
            match &mut result.meta {
                Some(meta) => {
                    meta.push_str("; ");
                    meta.push_str(&warning);
                }
                None => result.meta = Some(warning),
            }
        }
        result
    }

    /// Preserve parked rows and their terminal outcomes without acknowledging.
    pub async fn peek_inbox(&self, seat: &SeatId) -> Envelope<Vec<InboxClaim>> {
        self.execute(
            "pij inbox",
            self.http
                .get(format!("{}/v1/inbox", self.base_url))
                .query(&[("seat", seat.as_str()), ("peek", "true")]),
        )
        .await
    }

    /// Explicit operator recovery; the daemon resolves prime/parent authority.
    pub async fn release_inbox(
        &self,
        seat: &SeatId,
        job: u64,
        evidence: &str,
        caller: &CallerContext,
    ) -> Envelope<Value> {
        self.post(
            "pij inbox release",
            "/v1/inbox/release",
            &serde_json::json!({
                "seat":seat, "job_id":job, "evidence":evidence, "caller":caller,
            }),
        )
        .await
    }

    /// Open the federated event stream. `since` is encoded as one URL-encoded
    /// JSON object so every machine advances in its own sequence namespace.
    pub async fn tail(
        &self,
        since: &BTreeMap<String, u64>,
    ) -> std::result::Result<TailStream, Box<Envelope<Value>>> {
        let mut request = self.auth(self.http.get(format!("{}/v1/events", self.base_url)));
        if !since.is_empty() {
            let encoded = match serde_json::to_string(since) {
                Ok(encoded) => encoded,
                Err(error) => {
                    return Err(Box::new(refusal(
                        "pij tail",
                        ErrorKind::Adapter,
                        format!("could not encode cursor map: {error}"),
                    )));
                }
            };
            request = request.query(&[("since", encoded)]);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Err(Box::new(refusal(
                    "pij tail",
                    ErrorKind::Adapter,
                    format!("could not reach the daemon: {error}"),
                )));
            }
        };
        if !response.status().is_success() {
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => {
                    return Err(Box::new(refusal(
                        "pij tail",
                        ErrorKind::Adapter,
                        format!("could not read the daemon's refusal: {error}"),
                    )));
                }
            };
            return Err(Box::new(decode_response("pij tail", body)));
        }
        Ok(TailStream {
            response,
            buffer: Vec::new(),
            line_number: 0,
            saw_hello: false,
            ended: false,
        })
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.token),
        )
    }

    async fn get<T: DeserializeOwned>(&self, command: &str, path: &str) -> Envelope<T> {
        self.execute(command, self.http.get(format!("{}{path}", self.base_url)))
            .await
    }

    pub(crate) async fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        command: &str,
        path: &str,
        body: &B,
    ) -> Envelope<T> {
        self.execute(
            command,
            self.http
                .post(format!("{}{path}", self.base_url))
                .json(body),
        )
        .await
    }

    async fn execute<T: DeserializeOwned>(
        &self,
        command: &str,
        request: reqwest::RequestBuilder,
    ) -> Envelope<T> {
        let response = match self.auth(request).send().await {
            Ok(response) => response,
            Err(error) => {
                return refusal(
                    command,
                    ErrorKind::Adapter,
                    format!("could not reach the daemon: {error}"),
                );
            }
        };
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                return refusal(
                    command,
                    ErrorKind::Adapter,
                    format!("could not read the daemon's reply: {error}"),
                );
            }
        };
        decode_response(command, body)
    }
}

/// A live event stream. Each call returns one complete frame without buffering
/// the unbounded stream.
pub struct TailStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    line_number: usize,
    saw_hello: bool,
    ended: bool,
}

impl TailStream {
    /// Read the next event or peer-state frame.
    ///
    /// # Errors
    /// When the stream is unreadable, does not begin with a supported Hello
    /// line, or carries a malformed tagged frame.
    pub async fn next_frame(&mut self) -> std::result::Result<Option<StreamFrame>, TailError> {
        loop {
            if let Some(line) = self.take_line()? {
                self.line_number += 1;
                if line.trim().is_empty() {
                    continue;
                }
                if !self.saw_hello {
                    self.read_hello(&line)?;
                    self.saw_hello = true;
                    continue;
                }
                let frame = serde_json::from_str(&line).map_err(|error| TailError {
                    kind: ErrorKind::Adapter,
                    message: format!("line {} is not an event frame: {error}", self.line_number),
                })?;
                return Ok(Some(frame));
            }

            if self.ended {
                if !self.buffer.is_empty() {
                    self.buffer.push(b'\n');
                    continue;
                }
                if !self.saw_hello {
                    return Err(TailError {
                        kind: ErrorKind::Adapter,
                        message: "event stream ended before its Hello line".to_string(),
                    });
                }
                return Ok(None);
            }

            match self.response.chunk().await {
                Ok(Some(chunk)) => self.buffer.extend_from_slice(&chunk),
                Ok(None) => self.ended = true,
                Err(error) => {
                    return Err(TailError {
                        kind: ErrorKind::Adapter,
                        message: format!("event stream failed: {error}"),
                    });
                }
            }
        }
    }

    fn take_line(&mut self) -> std::result::Result<Option<String>, TailError> {
        let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let mut bytes: Vec<u8> = self.buffer.drain(..=newline).collect();
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| TailError {
                kind: ErrorKind::Adapter,
                message: format!("line {} is not UTF-8: {error}", self.line_number + 1),
            })
    }

    fn read_hello(&self, line: &str) -> std::result::Result<(), TailError> {
        #[derive(Deserialize)]
        struct Hello {
            hello: bool,
            v: u32,
            build: String,
        }

        let hello: Hello = serde_json::from_str(line).map_err(|error| TailError {
            kind: ErrorKind::Adapter,
            message: format!("event stream did not begin with Hello: {error}"),
        })?;
        if !hello.hello || hello.build.is_empty() {
            return Err(TailError {
                kind: ErrorKind::Adapter,
                message: "event stream did not begin with a complete Hello line".to_string(),
            });
        }
        if hello.v != pij_core::wire::EVENT_VERSION {
            return Err(TailError {
                kind: ErrorKind::Skew,
                message: format!(
                    "event stream v{} is unsupported; this build speaks v{} — upgrade pij",
                    hello.v,
                    pij_core::wire::EVENT_VERSION
                ),
            });
        }
        Ok(())
    }
}

impl std::fmt::Debug for TailStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TailStream")
            .field("line_number", &self.line_number)
            .field("saw_hello", &self.saw_hello)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

/// A typed stream failure suitable for an [`Envelope`] refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailError {
    /// Machine-readable category.
    pub kind: ErrorKind,
    /// Human-readable diagnosis.
    pub message: String,
}

impl std::fmt::Display for TailError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TailError {}

fn decode_response<T: DeserializeOwned>(command: &str, body: String) -> Envelope<T> {
    let untyped = match pij_core::wire::decode_envelope::<Value>(&body) {
        Ok(envelope) => envelope,
        Err(error) => {
            let kind = match error {
                pij_core::wire::WireError::FutureVersion { .. } => ErrorKind::Skew,
                _ => ErrorKind::Adapter,
            };
            return refusal(command, kind, error.to_string());
        }
    };
    if untyped.v != ENVELOPE_VERSION {
        return refusal(
            command,
            ErrorKind::Skew,
            format!(
                "envelope v{} is unsupported; this build speaks v{} — upgrade pij",
                untyped.v, ENVELOPE_VERSION
            ),
        );
    }

    match serde_json::from_str::<Envelope<T>>(&body) {
        Ok(mut envelope) => {
            // Move the exact buffer that passed both version and payload checks.
            envelope.raw_json = Some(body);
            envelope
        }
        Err(error) => refusal(
            command,
            ErrorKind::Adapter,
            format!("daemon returned the wrong payload shape: {error}"),
        ),
    }
}

fn retype_envelope<T, U>(envelope: Envelope<T>) -> Envelope<U> {
    Envelope {
        ok: envelope.ok,
        command: envelope.command,
        v: envelope.v,
        data: None,
        meta: envelope.meta,
        error: envelope.error,
        details: envelope.details,
        raw_json: envelope.raw_json,
    }
}

fn refusal<T>(command: &str, kind: ErrorKind, message: impl Into<String>) -> Envelope<T> {
    Envelope::refused(command, kind, message)
}

/// Convert a local setup error into the same structured surface as a daemon
/// refusal.
pub fn setup_refusal<T>(command: &str, error: PijError) -> Envelope<T> {
    refusal(command, ErrorKind::Adapter, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    use pij_core::model::{Destination, ENVELOPE_VERSION, ErrorKind, Harness};
    use serde_json::json;

    use super::{DaemonClient, Registration, SendRequest, StreamFrame, decode_response};

    /// Plan 160: `pij-rs list` asks the daemon for sizes on both of its paths
    /// (plain or `--json`, and with scope arguments such as `--here`).
    #[tokio::test]
    async fn list_requests_ask_the_daemon_for_sizes() {
        for scoped in [false, true] {
            let state_dir = pij_testkit::fresh_store::fresh_dir("cli-list-sizes");
            std::fs::write(state_dir.join("daemon.key"), "test-token").expect("write key");
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake daemon");
            let addr = listener.local_addr().expect("fake daemon address");
            let (request_tx, request_rx) = mpsc::channel();
            let body =
                r#"{"ok":true,"command":"pij list","v":2,"data":{"seats":[],"unavailable":[]}}"#;
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().expect("accept client");
                let mut request = [0_u8; 4096];
                let read = socket.read(&mut request).expect("read request");
                request_tx
                    .send(String::from_utf8_lossy(&request[..read]).into_owned())
                    .expect("send captured request");
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write response");
            });
            let client =
                DaemonClient::new(&state_dir, &addr.to_string()).expect("construct client");
            let reply = if scoped {
                let caller: pij_daemon::http::CallerContext =
                    serde_json::from_value(json!({})).expect("empty caller");
                client.list_scoped(&["--here".to_string()], &caller).await
            } else {
                client.list_sized().await
            };
            assert!(reply.ok, "{scoped}: {:?}", reply.meta);
            let request = request_rx.recv().expect("captured request");
            let line = request.lines().next().unwrap_or_default();
            let method = if scoped { "POST" } else { "GET" };
            assert!(
                line.starts_with(&format!("{method} /v1/seats?sizes=true ")),
                "{scoped}: {line}"
            );
            server.join().expect("fake daemon");
        }
    }

    #[test]
    fn registration_requires_pid_and_start_time_together() {
        let mut registration = Registration {
            supersedes: None,
            id: "seat".into(),
            harness: Harness::Omp,
            folder: "/tmp".to_string(),
            extension_build: None,
            extension_path: None,
            pane: None,
            pid: None,
            proc_start: None,
            spawn_id: None,
            model: None,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        };

        assert_eq!(registration.validate(), Ok(()));
        registration.pid = Some(42);
        assert!(registration.validate().is_err());
        registration.proc_start = Some(20260830112233);
        assert_eq!(registration.validate(), Ok(()));
        registration.pid = None;
        assert!(registration.validate().is_err());
    }

    #[test]
    fn send_serializes_destination_as_an_object_and_keeps_reply_correlation() {
        let request = SendRequest {
            from: "sender".into(),
            to: Destination {
                seat: "recipient".into(),
                machine: Some("other-host".to_string()),
            },
            body: "hello".to_string(),
            msg_id: "m-1".to_string(),
            in_reply_to: Some("m-0".to_string()),
            command: None,
            fyi: false,
            force: false,
            reason: None,
        };

        assert_eq!(
            serde_json::to_value(request).expect("serialize request"),
            json!({
                "from": "sender",
                "to": {"seat": "recipient", "machine": "other-host"},
                "body": "hello",
                "msg_id": "m-1",
                "in_reply_to": "m-0"
            })
        );
    }

    #[test]
    fn unsupported_envelope_version_is_refused_before_payload_typing() {
        let body = r#"{"ok":true,"command":"pij list","v":99,"data":"not a seat list"}"#;

        let envelope =
            decode_response::<Vec<pij_core::model::SeatDescriptor>>("pij list", body.to_owned());

        assert!(!envelope.ok);
        assert_eq!(envelope.error, Some(ErrorKind::Skew));
        assert!(
            envelope
                .meta
                .as_deref()
                .is_some_and(|meta| meta.contains("v99"))
        );
    }

    #[test]
    fn every_unrecognised_version_is_skew_not_only_future_versions() {
        let body = r#"{"ok":true,"command":"pij ping","v":0,"data":"unknown old shape"}"#;

        let envelope = decode_response::<serde_json::Value>("pij ping", body.to_owned());

        assert!(!envelope.ok);
        assert_eq!(envelope.error, Some(ErrorKind::Skew));
        assert_eq!(ENVELOPE_VERSION, 2);
    }

    #[tokio::test]
    async fn tail_reads_hello_then_frames_and_keeps_unknown_event_kinds() {
        let state_dir = pij_testkit::fresh_store::fresh_dir("cli-tail");
        std::fs::write(state_dir.join("daemon.key"), "test-token").expect("write key");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake daemon");
        let addr = listener.local_addr().expect("fake daemon address");
        let (request_tx, request_rx) = mpsc::channel();
        let body = concat!(
            "{\"hello\":true,\"v\":1,\"build\":\"test\"}\n",
            "{\"type\":\"event\",\"machine\":\"local\",\"cursor\":7,\"event\":{\"v\":1,\"at\":9,\"kind\":\"future.kind\",\"seat\":null,\"payload\":\"{}\"}}\n"
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept client");
            let mut request = [0_u8; 2048];
            let read = socket.read(&mut request).expect("read request");
            request_tx
                .send(String::from_utf8_lossy(&request[..read]).into_owned())
                .expect("send captured request");
            write!(
                socket,
                "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
        });

        let client = DaemonClient::new(&state_dir, &addr.to_string()).expect("construct client");
        let cursors = BTreeMap::from([("local".to_string(), 6)]);
        let mut tail = client.tail(&cursors).await.expect("open tail");
        let frame = tail
            .next_frame()
            .await
            .expect("read frame")
            .expect("one frame");
        let StreamFrame::Event {
            machine,
            cursor,
            event,
        } = frame
        else {
            panic!("expected event frame");
        };
        assert_eq!(machine, "local");
        assert_eq!(cursor, 7);
        assert_eq!(event.kind, "future.kind");
        assert_eq!(tail.next_frame().await.expect("read end"), None);

        let request = request_rx.recv().expect("captured request");
        assert!(
            request.starts_with("GET /v1/events?since=%7B%22local%22%3A6%7D HTTP/1.1"),
            "{request}"
        );
        assert!(
            request.contains("authorization: Bearer test-token"),
            "{request}"
        );
        server.join().expect("fake daemon exits");
        std::fs::remove_dir_all(state_dir).expect("remove test state");
    }
}
