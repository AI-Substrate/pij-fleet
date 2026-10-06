//! Identity routes (plan 114, u-identity): `adopt`, `whoami`, `phonehome`.
//!
//! # One rule holds this module together
//!
//! **Caller context is a CLAIM. Identity is DERIVED, or it is refused.**
//!
//! Everything the caller says arrives over HTTP from a process the daemon cannot
//! see. So a claimed pane is checked against tmux, a claimed pid is only ever
//! believed PAIRED with a start time the liveness port observes independently
//! (a pid alone is recycled at boot), and a claimed seat id can LOCATE a row but
//! can never CREATE one and can never win an argument with what tmux says. This
//! platform has already paid for the opposite: an unvalidated `PIJ_SESSION_ID`
//! minted phantom seats and let one seat write as another.
//!
//! # Two smaller rules that are not style
//!
//! 1. **Every refusal carries a decodable envelope, never a bare 404.** The
//!    generation shim classifies `404`/`405` with an undecodable body as "rs does
//!    not implement this route" and falls back to LEGACY. A bare refusal here
//!    would therefore not be a refusal at all — it would silently re-home the
//!    seat into the other store while every surface reported success.
//! 2. **A success answer is READ BACK FROM THE STORE.** The TS route this ports
//!    prints `adopted <id> … (pane %N, bound)` on a dissolved descriptor and
//!    persists nothing. Answering from a re-read makes that class of defect
//!    unrepresentable rather than merely tested-against.

use std::fmt::Write as _;

use std::net::SocketAddr;

use axum::Extension;
use axum::extract::{ConnectInfo, Json, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Liveness, SeatDescriptor, SeatId};
use pij_core::ports::SeatFilter;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    AppState, Registration, RegistrationResponse, allocate_memorable_id, envelope, internal,
    pane_harness_identity, refused,
};
use crate::registration::RegistrationError;

/// The caller's environment, forwarded by the shim as a FIXED ALLOWLIST.
///
/// THREE SPELLINGS, and the third one is a scar. The shim sends camelCase
/// (`CALLER_WIRE_KEYS`, generation-routing.ts:242); the env NAMES are
/// SCREAMING_SNAKE; the `pij-rs` CLI uses snake_case. All three are accepted.
///
/// This reader originally accepted only the last two, because the wire contract
/// was distributed as PROSE — a flat list of the ENV names — while the TS source
/// held PAIRS of (env name, wire name). Six of nine keys never met. The block
/// deserialized to ALL-NONE, `whoami` and `phonehome` could not name a seat, and
/// BOTH SUITES STAYED GREEN because each side asserted its own spelling. serde
/// drops unknown fields in silence, so nothing anywhere failed.
///
/// The lesson is not "add an alias". It is that a cross-runtime contract living
/// in prose has one definition per reader, each self-consistent. The contract now
/// lives in `crates/daemon/tests/fixtures/caller-context.wire.json`, which both
/// runtimes read, and both halves assert on POPULATED values so neither can pass
/// vacuously.
///
/// Every field is a CLAIM. None of them is trusted on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerContext {
    /// The seat the caller believes it is. LOCATES a row; never creates one.
    #[serde(
        default,
        rename = "PIJ_SESSION_ID",
        alias = "pij_session_id",
        alias = "pijSessionId"
    )]
    pub session_id: Option<String>,
    /// Generic harness-native session id used by pi and OMP callers.
    #[serde(
        default,
        rename = "HARNESS_SESSION_ID",
        alias = "harness_session",
        alias = "harnessSession"
    )]
    pub harness_session: Option<String>,
    /// The pane the caller believes it occupies. CHECKED against tmux.
    #[serde(default, rename = "TMUX_PANE", alias = "tmux_pane", alias = "tmuxPane")]
    pub pane: Option<String>,
    /// The governing seat, self-declared at adoption.
    #[serde(
        default,
        rename = "PIJ_PARENT_ID",
        alias = "pij_parent_id",
        alias = "pijParentId"
    )]
    pub parent: Option<String>,
    /// Harness-specific native session ids retained for existing callers.
    ///
    /// Registration stores the value for identity projection but never treats it
    /// as bind evidence; process identity remains the anti-impersonation control.
    #[serde(
        default,
        rename = "CLAUDE_CODE_SESSION_ID",
        alias = "claude_code_session_id",
        alias = "claudeCodeSessionId"
    )]
    pub claude_session: Option<String>,
    /// See `claude_session`.
    #[serde(
        default,
        rename = "COPILOT_AGENT_SESSION_ID",
        alias = "copilot_agent_session_id",
        alias = "copilotAgentSessionId"
    )]
    pub copilot_session: Option<String>,
    /// See `claude_session`.
    #[serde(
        default,
        rename = "CODEX_THREAD_ID",
        alias = "codex_thread_id",
        alias = "codexThreadId"
    )]
    pub codex_session: Option<String>,
    /// The caller's working folder. Identity uses it only when no pane can supply
    /// one; `bg create` runs its command here (Plan 163), as the TS CLI did.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Claimed pid — DIAGNOSTIC ONLY. **Never admissible as bind evidence.**
    ///
    /// Retained on the wire because it is useful in a refusal message and in a
    /// log, and removing it would only mean the next person re-adds it without
    /// this comment. But bind evidence is the ANTI-IMPERSONATION control, and a
    /// control whose input the subject supplies is not a control: a subagent
    /// inherits its parent's environment and can assert anything, while it
    /// cannot manufacture what the daemon OBSERVES.
    ///
    /// The routed shim forwarding its own short-lived process was merely the
    /// FIRST caller to notice a claim was accepted. The next one need not be
    /// short-lived, and need not be honest.
    ///
    /// `adopt_binds_the_pane_process_and_ignores_a_claimed_pid` pins this on the
    /// path that still exists, so re-opening paneless admission cannot quietly
    /// re-open this too.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Claimed process start stamp — DIAGNOSTIC ONLY, see [`Self::pid`].
    #[serde(default, rename = "procStart", alias = "proc_start")]
    pub proc_start: Option<u64>,
}

/// The body every identity route accepts.
///
/// `argv` is what the generation shim forwards verbatim; the explicit fields are
/// what the `pij-rs` CLI sends. Both are supported by the same handler so the
/// shim's fix is purely additive and neither caller is a second implementation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IdentityRequest {
    /// The typed command line, verb included.
    #[serde(default)]
    pub argv: Vec<String>,
    /// The caller's forwarded environment.
    #[serde(default)]
    pub caller: Option<CallerContext>,
    /// Explicit seat, for the read verbs.
    #[serde(default)]
    pub seat: Option<String>,
    /// Explicit pane, for the read verbs.
    #[serde(default)]
    pub pane: Option<String>,
    /// Loaded pij extension build supplied when adopting this runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_build: Option<String>,
    /// Real directory this runtime loaded its pij extension from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_path: Option<String>,
    /// A caller trying to NAME the seat. Refused — see `ac-1147`.
    #[serde(default)]
    pub id: Option<String>,
    /// Explicit adoption role; omission preserves and explicit null refuses.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::types::deserialize_asserted_role"
    )]
    pub role: Option<String>,
}

/// `?seat=&pane=` for the read verbs.
///
/// Accepted on GET as well as POST deliberately. The route table flips `whoami`
/// to POST, but a shim that has not yet been updated still sends GET — and axum
/// would answer that `405` with an EMPTY body, which the shim classifies as
/// route-absence and falls back to legacy. Serving both methods turns that
/// silent re-homing into a decodable answer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IdentityQuery {
    /// Asserted seat id. Locates a row; never creates one.
    #[serde(default)]
    pub seat: Option<String>,
    /// Pane to derive from. MUST be percent-encoded: every pane id starts with
    /// `%`, so a raw `?pane=%77` decodes to `pane=w`.
    #[serde(default)]
    pub pane: Option<String>,
}

/// What `phonehome` answers: a CONFIRMATION of a binding adopt already made.
#[derive(Debug, Serialize, Deserialize)]
pub struct Phonehome {
    /// The seat this confirms.
    pub seat: SeatId,
    /// Its harness, as the store holds it.
    pub harness: String,
    /// Its pane, when it has one.
    pub pane: Option<String>,
    /// Its current seat_roles assignment, never a descriptor's stale copy.
    pub role: Option<String>,
    /// Is the process this seat was bound to still the process that is running?
    ///
    /// NOT "did something answer". The pair `(pid, proc_start)` is re-observed:
    /// a recycled pid presents as a live process at the same number and a
    /// DIFFERENT start, and reporting that as bound is the false-live bug.
    pub bound: bool,
    /// The bound pid, when the seat is bound.
    pub pid: Option<u32>,
    /// The start stamp recorded at bind time.
    pub proc_start: Option<u64>,
    /// The start stamp observed NOW. Differs from `proc_start` exactly when the
    /// pid was recycled — the one case a pid-only check cannot see.
    pub observed_proc_start: Option<u64>,
    /// How this seat was identified, so a reader can tell a derived answer from
    /// a merely-existing one.
    pub resolved_by: String,
    /// What rs bound by. Constant, and stated rather than assumed: a caller
    /// arriving from the TS route expects a harness-native session id here.
    pub bound_by: String,
}

/// The bind evidence rs actually holds.
const BOUND_BY: &str = "process identity (pid paired with proc_start), observed by the daemon";

/// How a seat was identified for a read verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedBy {
    /// Matched against a pane, which the daemon can check.
    Pane,
    /// Matched a caller-asserted id that exists. Existence is all that was proven.
    AssertedId,
}

impl ResolvedBy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pane => "pane (derived)",
            Self::AssertedId => "asserted seat id (existence checked only)",
        }
    }
}

/// What an `adopt` command line said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AdoptArgs {
    /// The pane to adopt, when the pane form was used.
    pub pane: Option<String>,
    /// The declared harness.
    pub harness: Option<String>,
    /// The declared governing seat.
    pub parent: Option<String>,
    /// Role assertion; absence leaves the existing seat_roles row untouched.
    pub role: Option<String>,
    /// True for the paneless `inbox register` form the ready route uses when a
    /// seat has no `$TMUX_PANE`.
    pub paneless: bool,
    /// The harness-native conversation id, reported by a child of the pane's
    /// harness process (plan 156 ruling 1). Believed only after the daemon
    /// observes the calling connection descending from that process.
    pub harness_session: Option<String>,
    /// Operator reclaim target (plan 156 AC6).
    pub reclaim: Option<String>,
}

/// Parse the argv the shim forwards.
///
/// Pure, and separated from the handler so the shapes can be tested without a
/// daemon. An unknown flag is REFUSED BY NAME rather than ignored: silently
/// dropping `--session-id` would let a caller believe it had pinned a native
/// session that rs never stored.
pub(crate) fn parse_adopt_argv(argv: &[String]) -> Result<AdoptArgs, String> {
    let mut args = AdoptArgs::default();
    let mut tokens = argv.iter().map(String::as_str).peekable();
    match tokens.peek().copied() {
        Some("adopt") => {
            tokens.next();
        }
        Some("inbox") => {
            tokens.next();
            if tokens.next() != Some("register") {
                return Err(
                    "the only `inbox` form this route serves is `inbox register`".to_string(),
                );
            }
            args.paneless = true;
        }
        _ => {}
    }
    while let Some(token) = tokens.next() {
        match token {
            "--harness" => {
                args.harness = Some(
                    tokens
                        .next()
                        .ok_or_else(|| "--harness needs a value".to_string())?
                        .to_string(),
                );
            }
            "--parent" => {
                args.parent = Some(
                    tokens
                        .next()
                        .ok_or_else(|| "--parent needs a value".to_string())?
                        .to_string(),
                );
            }
            "--role" => {
                let role = tokens
                    .next()
                    .filter(|role| !role.trim().is_empty() && !role.starts_with("--"))
                    .ok_or_else(|| "--role needs a nonempty value".to_string())?;
                if args.role.replace(role.to_string()).is_some() {
                    return Err("--role may be supplied only once".to_string());
                }
            }
            "--harness-session" => {
                let session = tokens
                    .next()
                    .filter(|session| !session.trim().is_empty() && !session.starts_with("--"))
                    .ok_or_else(|| "--harness-session needs a nonempty value".to_string())?;
                args.harness_session = Some(session.to_string());
            }
            "--reclaim" => {
                let target = tokens
                    .next()
                    .filter(|target| !target.trim().is_empty() && !target.starts_with("--"))
                    .ok_or_else(|| "--reclaim needs a seat id".to_string())?;
                args.reclaim = Some(target.to_string());
            }
            // Output-shape flags. They change how a CLI prints, and this route
            // does not print — accepting them is honest, storing them is not.
            "--json" | "--export" => {}
            "--session-id" => {
                return Err(
                    "rs does not store a harness-native session id, so `--session-id` cannot be \
                     honoured — refusing by name rather than accepting it and differing silently"
                        .to_string(),
                );
            }
            flag if flag.starts_with("--") => {
                return Err(format!(
                    "`{flag}` is not an argument this route understands"
                ));
            }
            positional if args.pane.is_none() && !args.paneless => {
                args.pane = Some(positional.to_string());
            }
            positional => {
                return Err(format!("unexpected argument `{positional}`"));
            }
        }
    }
    Ok(args)
}

/// A tombstoned row is a POST-MORTEM, not an identity.
///
/// ONE definition, consulted by every path that resolves a seat, because the
/// defect this closes was an ASYMMETRY: the pane resolver already selected only
/// live rows while the asserted-id arm accepted any row `get` returned. A rule
/// spelled in one place and forgotten in another is how that happens.
pub(crate) fn is_live(seat: &SeatDescriptor) -> bool {
    seat.tombstoned_at.is_none()
}

/// Has this seat's recorded process gone (dead, or its pid recycled)?
///
/// Plan 156 rule 2: a pane id alone is neither identity nor liveness. Pane ids
/// and pids both reset at boot, so a row whose recorded `(pid, proc_start)` no
/// longer exists is not the thing running in its pane, even when that pane's
/// number exists again. Unknown evidence (no recorded process, a remote seat, a
/// failed probe) never disqualifies: this only removes answers, never adds them.
pub(crate) async fn process_gone(state: &AppState, seat: &SeatDescriptor) -> bool {
    let Some(proc) = seat.proc.filter(|_| seat.machine.is_none()) else {
        return false;
    };
    matches!(
        pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await,
        Ok(Liveness::Dead { .. } | Liveness::Recycled { .. })
    )
}

/// Refuse a dead seat, NAMING what killed it.
///
/// The reason matters more than the refusal. "No such seat" sends an operator
/// looking for a typo; "superseded at a native session boundary" tells them their
/// seat was replaced and which id to use instead.
///
/// Why this is not merely tidy: supersession tombstones a predecessor while
/// RETAINING its process identity, so a dead row can still name a pid that is
/// genuinely alive. `phonehome` would re-observe that live process and answer
/// `bound: true` for a seat nothing can address — identity and reporting claiming
/// success while delivery refuses the same recipient. The liveness of the process
/// makes the wrong answer more convincing, not more true.
pub(crate) async fn refuse_tombstoned(
    state: &AppState,
    command: &str,
    seat: &SeatDescriptor,
) -> Option<Response> {
    if is_live(seat) {
        return None;
    }
    let mut meta = format!(
        "seat `{}` is tombstoned and is not an identity: {}. A dead row can still name a live pid — supersession keeps the process identity — so confirming it would report a binding for a seat nothing can address.",
        seat.id,
        seat.tombstone_reason
            .as_deref()
            .unwrap_or("no reason recorded"),
    );
    let mut details = json!({ "seat": seat.id });
    // This is evidence attached to an existing refusal, never a new identity
    // ladder. Read one seat's latest tombstone; unrelated later rows cannot win.
    match state
        .services
        .spine
        .latest_matching(&seat.id, &["seat.tombstone"])
        .await
    {
        Ok(Some(event)) => match event.seq {
            Some(seq) => {
                details["tombstone_seq"] = json!(seq);
                write!(&mut meta, " Original tombstone event: spine {}.", seq.0)
                    .expect("writing to String cannot fail");
            }
            None => meta.push_str(" Historical tombstone event has no recorded sequence."),
        },
        Ok(None) => meta.push_str(" No historical tombstone event sequence is available."),
        Err(error) => {
            write!(&mut meta, " Tombstone history lookup failed: {error}.")
                .expect("writing to String cannot fail");
        }
    }
    let mut answer: Envelope<Value> = Envelope::refused(command, ErrorKind::Refused, meta);
    answer.details = Some(details);
    Some(envelope(StatusCode::BAD_REQUEST, &answer))
}

/// The `--reclaim` guards: caller authority, harness, and death. Returns the
/// authorised caller with the target row, or the refusal to send.
///
/// The refusal travels unboxed for the same reason [`Resolved`] keeps it inline:
/// it is returned straight to axum, once per request.
#[allow(clippy::result_large_err)]
async fn reclaim_target(
    state: &AppState,
    caller: &CallerContext,
    target: &str,
    harness: &str,
) -> Result<(SeatId, SeatDescriptor), Response> {
    let actor =
        match resolve_seat(state, ADOPT, caller.session_id.clone(), caller.pane.clone()).await {
            Resolved::Seat(actor, _) => actor.id,
            Resolved::Refusal(response) => return Err(response),
        };
    let seat = match state.services.registry.get(&SeatId::from(target)).await {
        Ok(Some(seat)) => seat,
        Ok(None) => {
            return Err(refused(
                ADOPT,
                format!("`{target}` names no seat to reclaim"),
            ));
        }
        Err(error) => return Err(internal(ADOPT, error)),
    };
    let authorized = seat.parent.as_ref() == Some(&actor)
        || match state.services.roles.read_role(&actor).await {
            Ok(role) => role.as_deref() == Some("prime"),
            Err(error) => return Err(internal(ADOPT, error)),
        };
    if !authorized {
        return Err(refused(
            ADOPT,
            format!(
                "`{actor}` may not reclaim `{target}`: only its recorded parent or a prime may"
            ),
        ));
    }
    if seat.harness.as_str() != harness {
        return Err(refused(
            ADOPT,
            format!(
                "`{target}` is a {} seat; a {harness} process cannot reclaim it (harness mismatch)",
                seat.harness
            ),
        ));
    }
    match seat.proc {
        Some(proc) => match pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await
        {
            Ok(Liveness::Active) => {
                return Err(refused(
                    ADOPT,
                    format!(
                        "`{target}` is live elsewhere: its process (pid {}) is still running, pane {} — adopt there instead",
                        proc.pid,
                        seat.pane.as_deref().unwrap_or("none")
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) => return Err(internal(ADOPT, error)),
        },
        None if is_live(&seat) => {
            return Err(refused(
                ADOPT,
                format!("`{target}` records no process, so its death cannot be established"),
            ));
        }
        None => {}
    }
    Ok((actor, seat))
}

/// Does the process on the far end of `peer` descend from (or equal) `host`?
///
/// Both facts are OBSERVED, never claimed: `lsof` names the process owning the
/// client end of this very connection, and one `ps` snapshot supplies the parent
/// chain. Any failure answers `false`, which refuses; it never admits.
fn caller_descends_from(peer: SocketAddr, host: u32) -> bool {
    let Some(mut pid) = peer_pid(peer) else {
        return false;
    };
    let Ok(output) = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid="])
        .env("LC_ALL", "C")
        .output()
    else {
        return false;
    };
    let parents: std::collections::HashMap<u32, u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    for _ in 0..64 {
        if pid == host {
            return true;
        }
        match parents.get(&pid) {
            Some(&parent) if parent > 1 && parent != pid => pid = parent,
            _ => return false,
        }
    }
    false
}

/// The pid owning the CLIENT end of `peer`: the fd whose name reads `peer->…`.
/// Matching the direction rather than excluding the daemon's own pid keeps this
/// correct when client and server share a process.
fn peer_pid(peer: SocketAddr) -> Option<u32> {
    let output = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP@{peer}"), "-Fpn"])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    let prefix = format!("{peer}->");
    let mut pid = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if line
            .strip_prefix('n')
            .is_some_and(|name| name.starts_with(&prefix))
        {
            return pid;
        }
    }
    None
}

const ADOPT: &str = "pij adopt";
const WHOAMI: &str = "pij whoami";
const PHONEHOME: &str = "pij phonehome";

/// `POST /v1/adopt` — register this seat, with rs minting the name.
pub(crate) async fn adopt(
    State(state): State<AppState>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(request): Json<IdentityRequest>,
) -> Response {
    if let Some(asserted) = request.id.as_deref() {
        // ac-1147: ONE MINTER, and it is here. `register` keeps the
        // caller-supplied case for callers that legitimately hold a name; adopt
        // never does, because two minters are two namespaces.
        return refused(
            ADOPT,
            format!(
                "adopt does not accept a caller-supplied id (`{asserted}`): rs mints the seat name. \
                 Use `register` for the explicit case."
            ),
        );
    }
    let caller = request.caller.unwrap_or_default();
    let args = match parse_adopt_argv(&request.argv) {
        Ok(args) => args,
        Err(reason) => return refused(ADOPT, reason),
    };
    let role = match (request.role, args.role) {
        (Some(_), Some(_)) => {
            return refused(
                ADOPT,
                "role must be supplied as a typed field or --role, not both",
            );
        }
        (typed, argv) => typed.or(argv),
    };
    if let Some(Err(reason)) = role
        .as_deref()
        .map(pij_core::orchestration::check_seat_role)
    {
        return super::role::RoleError::Invalid(reason).into_response(ADOPT);
    }
    let pane = request
        .pane
        .or(args.pane)
        .or_else(|| caller.pane.clone())
        .filter(|pane| !pane.trim().is_empty());
    let Some(harness) = args.harness.clone() else {
        return refused(
            ADOPT,
            "adopt needs `--harness <claude|copilot|codex|omp>` — the harness is declared, never guessed",
        );
    };

    // Derive folder and process identity. This is the whole difference between a
    // registration and an assertion.
    let (folder, proc, proc_source) = if let Some(pane) = pane.as_deref() {
        let observed = match state.services.tmux.pane_process(pane).await {
            Ok(observed) => observed,
            Err(error) => return internal(ADOPT, error),
        };
        let Some(observed) = observed else {
            return refused(
                ADOPT,
                format!(
                    "pane `{pane}` is not running anything tmux can see — the descriptor is \
                     dissolved and NOTHING was written. Re-run from a live pane, or \
                     `pij-rs revive` the seat onto this one."
                ),
            );
        };
        let Some(kind) = pij_core::model::Harness::parse(&harness) else {
            return refused(ADOPT, format!("unknown harness `{harness}`"));
        };
        let identity = match pane_harness_identity(&state, observed.pid, kind).await {
            Ok(identity) => identity,
            Err(error) => return internal(ADOPT, error),
        };
        let Some(binding) = identity else {
            return refused(
                ADOPT,
                format!(
                    "tmux reports pid {} on pane `{pane}` but the process is already gone — \
                     nothing was written",
                    observed.pid
                ),
            );
        };
        (observed.cwd, Some(binding.proc), binding.source)
    } else {
        // F005 — ADOPT DERIVES ITS EVIDENCE; `register` ACCEPTS AN ASSERTED ONE.
        // PANELESS REGISTRATION IS REFUSED OUTRIGHT.
        //
        // This branch used to admit a seat on a caller-supplied
        // `(pid, proc_start)` pair, and it was right about what it DEMANDED: the
        // pair, corroborated against the liveness port, no admission without
        // bind evidence. The defect was one level up. Bind evidence is the
        // ANTI-IMPERSONATION control, and a control whose input the SUBJECT
        // SUPPLIES is not a control — a subagent inherits its parent's
        // environment and can assert anything, while it cannot manufacture what
        // the daemon observes for itself.
        //
        // The routed shim forwarding its own short-lived Node process was
        // therefore never the root cause; it was the first caller to notice a
        // claim was accepted. The next one need not be short-lived, and need not
        // be honest. There is no check that separates "the caller's process"
        // from "the seat's process" over HTTP — the daemon cannot see who
        // dialled it — so the honest boundary is the VERB, not a cleverer test.
        //
        // Paneless admission is UNSOLVED, carved out as req-0015; designing an
        // evidence rule for it is that row's work. Accepting a claim meanwhile
        // is not a stopgap, it is the control switched off.
        //
        // Second of two independent refusals by design: the shim refuses a
        // direct paneless `adopt` before routing. A control this plan has
        // already re-opened once does not get to rest on a single guard.
        return refused(
            ADOPT,
            "adopt registers a seat from evidence the daemon can DERIVE, and with no pane there \
             is nothing to derive from. A caller-supplied pid is not accepted here at all: bind \
             evidence exists to stop a process claiming to be a seat it is not, so a pid the \
             caller asserts cannot be the thing that proves it. Over HTTP the daemon also cannot \
             tell the caller's process from the seat's, and a routed adopt would bind the seat \
             to the short-lived CLI process that made the request. Use `register` for a paneless \
             seat: it takes an explicit id and an explicit process identity from a caller that \
             knows its own.",
        );
    };

    // Plan 156 AC6: an operator reclaims a dead seat whose continuity evidence
    // is gone. Every guard refuses before anything is written.
    let reclaim = match args.reclaim.as_deref() {
        None => None,
        Some(_) if args.harness_session.is_some() => {
            return refused(
                ADOPT,
                "`--reclaim` is an operator override and cannot also carry `--harness-session`",
            );
        }
        Some(target) => match reclaim_target(&state, &caller, target, &harness).await {
            Ok(guarded) => Some(guarded),
            Err(response) => return response,
        },
    };

    // Plan 156 rulings 1 and 2: a session is DERIVED, never taken on the
    // caller's word. For Claude the pane process's own sessions/<pid>.json record
    // names it on every adopt, and `--harness-session` only cross-checks it.
    // Other harnesses may pass it only from inside the pane's process tree.
    let kind = pij_core::model::Harness::parse(&harness);
    let derived_session = if kind == Some(pij_core::model::Harness::Claude) {
        let host = proc.expect("a paned adopt has a process identity");
        let homes = std::sync::Arc::clone(&state.services.claude_homes);
        let recorded = tokio::task::spawn_blocking(move || {
            let offset = pij_harnesses::proc::local_utc_offset_minutes().ok()?;
            pij_harnesses::proc::claude_session_of(&homes, host, offset)
        })
        .await
        .ok()
        .flatten();
        match (recorded, args.harness_session.clone()) {
            (Some(recorded), Some(claimed)) if recorded != claimed => {
                return refused(
                    ADOPT,
                    format!(
                        "`--harness-session {claimed}` does not match the conversation Claude \
                         records for pid {} (`{recorded}`) — nothing was written",
                        host.pid
                    ),
                );
            }
            (None, Some(claimed)) => {
                return refused(
                    ADOPT,
                    format!(
                        "`--harness-session {claimed}` cannot be corroborated: no Claude record \
                         for pid {} with a matching start exists — nothing was written",
                        host.pid
                    ),
                );
            }
            (recorded, _) => recorded,
        }
    } else {
        match args.harness_session.clone() {
            None => None,
            Some(session) => {
                let host = proc.expect("a paned adopt has a process identity").pid;
                let descends = match connection {
                    Some(Extension(ConnectInfo(peer))) => {
                        tokio::task::spawn_blocking(move || caller_descends_from(peer, host))
                            .await
                            .unwrap_or(false)
                    }
                    None => false,
                };
                if !descends {
                    return refused(
                        ADOPT,
                        format!(
                            "`--harness-session {session}` is accepted only from inside the pane's \
                         harness process tree (pid {host}), and the calling process is not in \
                         it — nothing was written. Run adopt from the harness's status line or \
                         SessionStart hook in that pane."
                        ),
                    );
                }
                Some(session)
            }
        }
    };

    let seats = match state.services.registry.list(SeatFilter::default()).await {
        Ok(seats) => seats,
        Err(error) => return internal(ADOPT, error),
    };
    // A seat follows its conversation (plan 156 rule 1): the seat already holding
    // this session, live or retired, outranks whatever the pane number says.
    let conversation = derived_session.as_deref().and_then(|session| {
        seats
            .iter()
            .filter(|seat| {
                Some(seat.harness) == kind && seat.harness_session.as_deref() == Some(session)
            })
            .min_by_key(|seat| {
                (
                    !is_live(seat),
                    std::cmp::Reverse(seat.proc.map(|proc| proc.proc_start)),
                    seat.id.clone(),
                )
            })
    });
    if let Some(seat) = conversation
        && is_live(seat)
        && seat.proc.is_some()
        && seat.proc != proc
        && !process_gone(&state, seat).await
    {
        return refused(
            ADOPT,
            format!(
                "conversation `{}` is live as seat `{}` in another process (pane {}) — adopt \
                 there instead; one conversation cannot be two seats",
                derived_session.as_deref().unwrap_or_default(),
                seat.id,
                seat.pane.as_deref().unwrap_or("none"),
            ),
        );
    }
    // Adopt INTO the seat that already owns this pane rather than minting a
    // second id for it. The TS route grew this the hard way (its plan-071 defect
    // B): without it, a re-adopt leaves two descriptors for one pane and peers
    // address the dead one.
    // A row whose recorded process is gone is not this pane's incumbent, however
    // its pane number reads (plan 156 rule 2).
    let mut incumbent = None;
    for seat in seats
        .iter()
        .filter(|seat| is_live(seat) && seat.pane.is_some() && seat.pane == pane)
    {
        if !process_gone(&state, seat).await {
            incumbent = Some(seat);
            break;
        }
    }
    let existing = reclaim
        .as_ref()
        .map(|(_, target)| target)
        .or(conversation)
        .or(incumbent);
    let seat_id = match existing {
        Some(seat) => seat.id.clone(),
        None => {
            let seed = format!(
                "adopt:{harness}:{}:{}",
                pane.as_deref().unwrap_or("paneless"),
                proc.map_or(0, |proc| proc.pid)
            );
            match allocate_memorable_id(&*state.services.registry, &seed).await {
                Ok(Some(id)) => id,
                Ok(None) => {
                    return refused(ADOPT, "every memorable name is taken");
                }
                Err(error) => return internal(ADOPT, error),
            }
        }
    };

    // req-0008: an omitted argument means UNSAID, not CLEARED. `register`
    // overwrites `parent` and `relay` from the claim, so adopt carries the
    // incumbent's values forward when the command line was silent about them.
    // A reclaim keeps the target's parent: the operator's own is not the seat's.
    let parent = args
        .parent
        .or_else(|| caller.parent.clone())
        .map(SeatId::from)
        .filter(|_| reclaim.is_none())
        .or_else(|| existing.and_then(|seat| seat.parent.clone()));
    let harness_session = derived_session
        .clone()
        .or_else(|| caller.harness_session.clone())
        .or_else(|| match harness.as_str() {
            "claude" => caller.claude_session.clone(),
            "copilot" => caller.copilot_session.clone(),
            "codex" => caller.codex_session.clone(),
            _ => None,
        });
    // The pane's previous registration for this very process yields to the
    // conversation's seat, rather than colliding with it.
    let supersedes = existing.and(incumbent).and_then(|incumbent| {
        (incumbent.id != seat_id && incumbent.proc.is_some() && incumbent.proc == proc)
            .then(|| incumbent.id.clone())
    });
    let claim = Registration {
        supersedes,
        id: seat_id.0.clone(),
        harness,
        folder,
        extension_build: request.extension_build,
        extension_path: request.extension_path,
        pane: pane.clone(),
        pid: proc.map(|proc| proc.pid),
        proc_start: proc.map(|proc| proc.proc_start),
        spawn_id: None,
        model: None,
        actual_model: None,
        actual_model_observed: false,
        provider: None,
        effort: None,
        parent,
        role,
        relay: existing.is_some_and(|seat| seat.relay),
    };
    let registration = &state.registration;
    let registered = match &reclaim {
        Some(_) => registration.reclaim(claim).await,
        None => registration
            .register_with_harness_session(claim, harness_session)
            .await
            .map(|(descriptor, _)| descriptor),
    };
    let registered = match registered {
        Ok(registered) => registered,
        Err(error) => return adopt_registration_error(error),
    };
    if let Some((actor, target)) = &reclaim {
        let event = pij_core::model::Event {
            seq: None,
            v: pij_core::wire::EVENT_VERSION,
            at: match super::system_time_ms() {
                Ok(at) => at,
                Err(error) => return internal(ADOPT, error),
            },
            kind: "seat.reclaimed".into(),
            seat: Some(target.id.clone()),
            payload: json!({
                "caller": actor,
                "harness": target.harness,
                "old_proc": target.proc, "new_proc": registered.proc,
                "old_pane": target.pane, "new_pane": registered.pane,
                "old_harness_session": target.harness_session,
                "prior_reason": target.tombstone_reason,
                "evidence": "target process observed gone; caller is its recorded parent or a prime",
            })
            .to_string(),
        };
        if let Err(error) = state.services.event_bus.publish(event).await {
            return internal(ADOPT, error);
        }
    }

    // THE ANSWER IS A RE-READ. Not the descriptor we just built — the row the
    // store actually holds. A success line that persists nothing cannot be
    // expressed from here.
    match state.services.registry.get(&seat_id).await {
        Ok(Some(persisted)) => {
            let persisted = match state.services.roles.project_seat(persisted).await {
                Ok(persisted) => persisted,
                Err(error) => return internal(ADOPT, error),
            };
            let mut answer = Envelope::ok(
                ADOPT,
                RegistrationResponse {
                    descriptor: persisted,
                    binding: None,
                    proc_source: Some(proc_source),
                    // An adopted seat runs the same extension as a registered
                    // one and must hear the same grace; answering 0 here would
                    // make adopt the one path that steps on a typing human.
                    typing_grace_ms: Some(super::resolve_typing_grace_ms()),
                },
            );
            answer.meta = Some(format!(
                "adopted {seat_id}; identity resolves from pane {} on each command — no PIJ_SESSION_ID export is required for this harness",
                pane.as_deref()
                    .expect("paneless adopt returned before registration")
            ));
            envelope(StatusCode::OK, &answer)
        }
        Ok(None) => internal(
            ADOPT,
            format!(
                "registration reported success but no row for `{seat_id}` exists — refusing to \
                 report an adoption that did not persist"
            ),
        ),
        Err(error) => internal(ADOPT, error),
    }
}

fn adopt_registration_error(error: RegistrationError) -> Response {
    match error {
        RegistrationError::Refused(reason) => refused(ADOPT, reason),
        RegistrationError::Retryable(reason) => {
            let mut answer: Envelope<()> = Envelope::refused(ADOPT, ErrorKind::Refused, reason);
            answer.details = Some(json!({ "retryable": true }));
            envelope(StatusCode::CONFLICT, &answer)
        }
        error @ RegistrationError::NativeSessionHold(_) => {
            let mut answer: Envelope<()> =
                Envelope::refused(ADOPT, ErrorKind::Refused, error.to_string());
            answer.details = Some(json!({ "retryable": true, "hold": "native-session" }));
            envelope(StatusCode::CONFLICT, &answer)
        }
        RegistrationError::Runtime(error) => internal(ADOPT, error),
    }
}

/// `GET|POST /v1/whoami` — name the seat this caller is, or refuse.
pub(crate) async fn whoami_get(
    State(state): State<AppState>,
    Query(query): Query<IdentityQuery>,
) -> Response {
    whoami_resolved(state, query.seat, query.pane).await
}

/// See [`whoami_get`]. The POST arm is the canonical one once the shim forwards
/// caller context.
pub(crate) async fn whoami_post(
    State(state): State<AppState>,
    Json(request): Json<IdentityRequest>,
) -> Response {
    let caller = request.caller.unwrap_or_default();
    whoami_resolved(
        state,
        request.seat.or(caller.session_id),
        request.pane.or(caller.pane),
    )
    .await
}

async fn whoami_resolved(
    state: AppState,
    asserted: Option<String>,
    pane: Option<String>,
) -> Response {
    match resolve_seat(&state, WHOAMI, asserted, pane).await {
        Resolved::Seat(descriptor, _) => {
            let descriptor = match state.services.roles.project_seat(descriptor).await {
                Ok(descriptor) => descriptor,
                Err(error) => return internal(WHOAMI, error),
            };
            // The status lines already call whoami once per render; the FYI count
            // rides on it so `✉N` costs no second request (plan 158).
            let pending_fyis = match state
                .services
                .delivery
                .pending_fyi_count(&descriptor.id)
                .await
            {
                Ok(count) => count,
                Err(error) => return internal(WHOAMI, error),
            };
            envelope(
                StatusCode::OK,
                &Envelope::ok(
                    WHOAMI,
                    WhoamiProjection {
                        descriptor,
                        pending_fyis,
                    },
                ),
            )
        }
        Resolved::Refusal(response) => response,
    }
}

/// The seat, plus the count of FYIs held for it.
#[derive(serde::Serialize)]
struct WhoamiProjection {
    #[serde(flatten)]
    descriptor: SeatDescriptor,
    pending_fyis: u64,
}

/// The identity ladder's answer.
///
/// Kept inline deliberately: boxing [`SeatDescriptor`] would allocate on every
/// successful identity resolution, which is the hot path. The size imbalance is
/// cheaper than adding that per-request heap allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Resolved {
    /// The seat, and what identified it.
    Seat(SeatDescriptor, ResolvedBy),
    /// The refusal to send back, already an envelope.
    Refusal(Response),
}

/// `POST /v1/phonehome` — confirm the binding adopt already made.
pub(crate) async fn phonehome(
    State(state): State<AppState>,
    Query(query): Query<IdentityQuery>,
    Json(request): Json<IdentityRequest>,
) -> Response {
    let caller = request.caller.unwrap_or_default();
    let asserted = request.seat.or(query.seat).or(caller.session_id);
    let pane = request.pane.or(query.pane).or(caller.pane);
    let (descriptor, resolved_by) = match resolve_seat(&state, PHONEHOME, asserted, pane).await {
        Resolved::Seat(descriptor, resolved_by) => (descriptor, resolved_by),
        Resolved::Refusal(response) => return response,
    };
    // Re-observe rather than re-derive. `adopt` bound this seat synchronously, so
    // there is nothing to poll for: the only open question is whether the process
    // it bound is still the process that is there.
    let observed = match descriptor.proc {
        Some(proc) => match state.services.liveness.proc_start(proc.pid).await {
            Ok(observed) => observed,
            Err(error) => return internal(PHONEHOME, error),
        },
        None => None,
    };
    let bound = descriptor
        .proc
        .is_some_and(|proc| observed == Some(proc.proc_start));
    // A caller that sent a harness-native session id is told plainly that rs did
    // not bind by it. Silence here would read as agreement.
    let claimed_harness_session = caller
        .harness_session
        .as_deref()
        .or(caller.claude_session.as_deref())
        .or(caller.copilot_session.as_deref())
        .or(caller.codex_session.as_deref())
        .map(str::trim)
        .filter(|session| !session.is_empty());
    let descriptor = match state.services.roles.project_seat(descriptor).await {
        Ok(descriptor) => descriptor,
        Err(error) => return internal(PHONEHOME, error),
    };
    let mut answer = Envelope::ok(
        PHONEHOME,
        Phonehome {
            seat: descriptor.id.clone(),
            harness: descriptor.harness.to_string(),
            pane: descriptor.pane.clone(),
            role: descriptor.role,
            bound,
            pid: descriptor.proc.map(|proc| proc.pid),
            proc_start: descriptor.proc.map(|proc| proc.proc_start),
            observed_proc_start: observed,
            resolved_by: resolved_by.as_str().to_string(),
            bound_by: BOUND_BY.to_string(),
        },
    );
    if let Some(session) = claimed_harness_session {
        answer.meta = Some(format!(
            "the caller supplied harness session `{session}`; rs stores it for identity projection but did not bind by it — this confirmation is process identity, not a native-session pin"
        ));
    }
    envelope(StatusCode::OK, &answer)
}

/// The identity ladder, shared by the read verbs AND by `report` (plan 117).
///
/// It stopped being read-only when `report` began calling it, and that is
/// deliberate: `report` previously carried its own private resolution that read
/// an asserted id and nothing else, so it was the one routed verb whose subject
/// could not be DERIVED. Two ladders would be two places for the
/// anti-impersonation posture below to drift apart, which this repo has already
/// paid for elsewhere. A caller reaching this from a WRITE verb gets the same
/// rule the read verbs get — an asserted id never outranks an observable pane —
/// which makes the write STRICTER than it was, not looser.
///
/// Strictest evidence first. A pane can be checked against tmux and the roster;
/// an asserted id can only be checked for EXISTENCE. When both are present and
/// they disagree, the assertion loses — that disagreement is what impersonation
/// looks like, and preferring the claim is how one seat comes to answer as
/// another.
pub(crate) async fn resolve_seat(
    state: &AppState,
    command: &str,
    asserted: Option<String>,
    pane: Option<String>,
) -> Resolved {
    if asserted.as_deref() == Some(pij_core::BG_ACTOR) {
        return Resolved::Refusal(refused(
            command,
            "pij-bg is a daemon-owned sender, not a caller seat",
        ));
    }
    let asserted = asserted.filter(|id| !id.trim().is_empty());
    let pane = pane.filter(|pane| !pane.trim().is_empty());
    if asserted.is_none() && pane.is_none() {
        return Resolved::Refusal(refused(
            command,
            "no caller context reached rs, so there is no seat to name. This route needs a pane \
             (`?pane=%N`, or `caller.TMUX_PANE`) or a seat id (`?seat=<id>`, or \
             `caller.PIJ_SESSION_ID`). Refusing rather than guessing: with one seat in the store \
             a guess would even look right.",
        ));
    }
    if let Some(pane) = pane.as_deref() {
        let mut seats = match state.services.registry.list(SeatFilter::default()).await {
            Ok(seats) => seats,
            Err(error) => return Resolved::Refusal(internal(command, error)),
        };
        let mut found = None;
        let mut gone = None;
        for (index, seat) in seats.iter().enumerate() {
            if !is_live(seat) || seat.pane.as_deref() != Some(pane) {
                continue;
            }
            if process_gone(state, seat).await {
                gone.get_or_insert(index);
            } else {
                found = Some(index);
                break;
            }
        }
        let seat = match found {
            Some(index) => seats.swap_remove(index),
            None if let Some(index) = gone => {
                let seat = &seats[index];
                let proc = seat.proc.expect("a gone seat recorded a process");
                return Resolved::Refusal(refused(
                    command,
                    format!(
                        "seat `{}` records pane `{pane}`, but its process (pid {}, start {}) is gone — \
                         the pane id was reused, so this pane is not that seat. Adopt this pane \
                         (`pij adopt \"$TMUX_PANE\" --harness <h>`); a resumed conversation gets \
                         its own seat back.",
                        seat.id, proc.pid, proc.proc_start
                    ),
                ));
            }
            None => {
                // A pane's unambiguous retired history can explain a refusal,
                // but cannot authenticate its caller or choose a replacement.
                let mut history = seats
                    .iter()
                    .filter(|seat| !is_live(seat) && seat.pane.as_deref() == Some(pane));
                let retired = match (history.next(), history.next()) {
                    (Some(retired), None)
                        if asserted
                            .as_deref()
                            .is_none_or(|id| id == retired.id.as_str()) =>
                    {
                        Some(retired)
                    }
                    _ => None,
                };
                if let Some(retired) = retired
                    && let Some(refusal) = refuse_tombstoned(state, command, retired).await
                {
                    return Resolved::Refusal(refusal);
                }
                return Resolved::Refusal(refused(
                    command,
                    format!(
                        "no live seat in the rs store owns pane `{pane}` — this seat is not registered \
                         here. Run `pij adopt \"$TMUX_PANE\" --harness <h>` first."
                    ),
                ));
            }
        };
        if seat.id.0 == pij_core::BG_ACTOR {
            return Resolved::Refusal(refused(
                command,
                "pij-bg is a daemon-owned sender, not a caller seat",
            ));
        }
        if let Some(asserted) = asserted.as_deref()
            && asserted != seat.id.0
        {
            return Resolved::Refusal(refused(
                command,
                format!(
                    "the caller claims to be `{asserted}` but pane `{pane}` belongs to `{}` — \
                     refusing rather than answering for either. An asserted id never outranks an \
                     observable one. Retry with `env -u PIJ_SESSION_ID` so the pane-derived identity can be used.",
                    seat.id
                ),
            ));
        }
        return Resolved::Seat(seat, ResolvedBy::Pane);
    }
    let asserted = SeatId::from(asserted.expect("one of the two is present"));
    match state.services.registry.get(&asserted).await {
        Ok(Some(seat)) => match refuse_tombstoned(state, command, &seat).await {
            Some(refusal) => Resolved::Refusal(refusal),
            None => Resolved::Seat(seat, ResolvedBy::AssertedId),
        },
        // NOT a mint, and not a 404: an id that names nothing is a refusal with a
        // reason, and the store is untouched.
        Ok(None) => Resolved::Refusal(refused(
            command,
            format!(
                "`{asserted}` names no seat in the rs store. An asserted id is a claim, never an \
                 identity — nothing was created for it."
            ),
        )),
        Err(error) => Resolved::Refusal(internal(command, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adopt_registration_retryable_error_preserves_conflict_discriminator() {
        // Adopt currently uses ordinary registration, not native attestation.
        // Exercise its error projection, without claiming this race is reachable
        // through today's adopt request.
        let response = adopt_registration_error(RegistrationError::Retryable(
            "native owner changed during death observation; retry registration".into(),
        ));
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let answer: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(answer["v"], 2);
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["command"], "pij adopt");
        assert_eq!(answer["error"], "refused");
        assert_eq!(answer["details"]["retryable"], true);
        assert!(answer.get("data").is_none());
    }
}
