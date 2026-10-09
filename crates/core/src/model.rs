//! The domain, as types.
//!
//! Everything here is data plus the rules that are true of it — no IO, no
//! runtime, no adapters. If a decision can be made from these types alone, it
//! belongs in this crate and gets tested with zero doubles.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A seat's stable identity — the name every other surface addresses it by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SeatId(pub String);

impl SeatId {
    /// Borrow the underlying id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SeatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SeatId {
    fn from(value: &str) -> Self {
        SeatId(value.to_string())
    }
}

/// Which agent harness a seat runs.
///
/// `Omp` is a distinct variant rather than an alias of `Pi`: they share a
/// lineage but not their session artifacts, and TS defect #3 (subagents
/// registering as seats) turned on telling them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    /// Anthropic Claude Code.
    Claude,
    /// GitHub Copilot CLI.
    Copilot,
    /// OpenAI Codex CLI.
    Codex,
    /// Pi.
    Pi,
    /// Oh My Pi.
    Omp,
}

impl Harness {
    /// The lowercase wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Copilot => "copilot",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Omp => "omp",
        }
    }

    /// Parse the wire spelling. `None` for anything else — an unknown harness is
    /// a fact to report, never a silent default (the TS CLI guessed `pi` here and
    /// bound seats to the wrong readiness anchor).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Harness::Claude),
            "copilot" => Some(Harness::Copilot),
            "codex" => Some(Harness::Codex),
            "pi" => Some(Harness::Pi),
            "omp" => Some(Harness::Omp),
            _ => None,
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// Keep each axis's variants, wire spellings and published vocabulary together.
macro_rules! state_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(#[$first_meta:meta])*
            $first:ident => $first_word:literal,
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $word:literal,
            )*
        }
        $(#[$parse_meta:meta])*
        fn parse;
        $(words: $words:ident;)?
    ) => {
        $(#[$enum_meta])*
        pub enum $name {
            $(#[$first_meta])*
            #[serde(rename = $first_word)]
            $first,
            $(
                $(#[$variant_meta])*
                #[serde(rename = $word)]
                $variant,
            )*
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[Self::$first, $(Self::$variant,)*];

            state_enum!(@words [$($words)?] $first_word $(, $word)*);

            /// The stable, lowercase wire spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    Self::$first => $first_word,
                    $(Self::$variant => $word,)*
                }
            }

            $(#[$parse_meta])*
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $first_word => Some(Self::$first),
                    $($word => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
    (@words [$words:ident] $first:literal $(, $word:literal)*) => {
        /// Accepted wire words, pipe-separated in declaration order.
        pub const $words: &'static str = concat!($first $(, "|", $word)*);
    };
    (@words [] $first:literal $(, $word:literal)*) => {};
}

state_enum! {
    /// What the machine can observe about a seat: is its process doing work.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum SystemState {
        /// No inference in flight.
        Idle => "idle",
        /// A turn is running.
        Working => "working",
        /// The process is gone or the seat has been tombstoned.
        Dead => "dead",
        /// Pre-bind lifecycle; reserved until mechanical observation is published.
        Starting => "starting",
        /// A working observation has gone silent; reserved until activity is published.
        Stalled => "stalled",
        /// The process is suspended; reserved until suspension is observed.
        Stopped => "stopped",
        /// Mechanical observation is unavailable.
        Unknown => "unknown",
    }
    /// Parse a wire spelling without guessing an unknown observation.
    fn parse;
}

state_enum! {
    /// What the seat SAYS about itself. Observation and declaration are different
    /// facts and are stored separately: a seat that is idle because it is waiting
    /// for a human answer is not the same as one that has wedged, and collapsing the
    /// two is what made the TS watchdog nudge parked seats (defect #9).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum SemanticState {
        /// Adopted, idle, available for dispatch.
        Ready => "ready",
        /// Waiting on a peer or a dispatch.
        Waiting => "waiting",
        /// Deliberately parked by its operator.
        Hold => "hold",
        /// Waiting on an external dependency it cannot influence.
        Blocked => "blocked",
        /// Waiting on a human answer.
        Question => "question",
        /// Work finished.
        Done => "done",
        /// Work ended unsuccessfully.
        Failed => "failed",
        /// Work was cancelled.
        Cancelled => "cancelled",
    }
    /// Parse a declared state without substituting a near-fit meaning.
    fn parse;
    words: WORDS;
}

impl SemanticState {
    /// May the watchdog nudge a seat in this state?
    ///
    /// Deliberate silence is not a stall. This is the predicate TS lacked; the
    /// decision table in `pij-core`'s tests is its regression proof.
    pub const fn nudgeable(self) -> bool {
        match self {
            // Only a seat that declared itself available for work.
            SemanticState::Ready => true,
            // `Waiting` is on this side of the line by RULING (services.dd.md:
            // "eligible(seat) excludes waiting|hold|blocked|question"), and the
            // ruling is right: a seat waiting on a peer or a dispatch is silent
            // for a reason it already told us, and nudging it asks a question it
            // has answered. An earlier draft here had Waiting nudgeable and a
            // test that locked the contradiction in — caught in review, 2026-08-28.
            SemanticState::Waiting
            | SemanticState::Hold
            | SemanticState::Blocked
            | SemanticState::Question
            | SemanticState::Done
            | SemanticState::Failed
            | SemanticState::Cancelled => false,
        }
    }
}

/// A process identity that survives pid recycling.
///
/// A pid alone is NOT an identity: the OS reissues it, and a recycled pid made
/// the TS `revive` refuse live seats and accept dead ones. The pair (pid, start
/// time) is unique for the life of the machine's boot, so every liveness verdict
/// in this workspace is keyed on the pair or it is not a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcIdentity {
    /// Process id.
    pub pid: u32,
    /// Process start time, in whatever monotonic-per-boot unit the platform
    /// adapter reports. Compared, never interpreted.
    pub proc_start: u64,
}

/// Parse C-locale process-start fields into a `YYYYMMDDhhmmss` identity.
///
/// This encodes the supplied wall-time fields without timezone conversion or
/// clock access. Native `ps` observations are local time; Claude's `procStart`
/// records are UTC. Callers comparing them must explicitly convert timebases or
/// observe the process in UTC before parsing.
///
/// # Errors
/// Returns an adapter error when the row is not `Www Mmm DD HH:MM:SS YYYY`
/// with a valid date in 1970..=9999 and a valid time. The weekday is required
/// but its spelling and agreement with the date are not validated.
pub fn parse_process_start(row: &str) -> crate::error::Result<u64> {
    let mut fields = row.split_whitespace();
    let weekday = fields.next();
    let month = fields.next().and_then(parse_month);
    let day = fields.next().and_then(|value| value.parse::<u32>().ok());
    let time = fields.next().and_then(parse_time);
    let year = fields.next().and_then(|value| value.parse::<u32>().ok());

    let Some((month, day, (hour, minute, second), year)) = month
        .zip(day)
        .zip(time)
        .zip(year)
        .map(|(((m, d), t), y)| (m, d, t, y))
    else {
        return Err(parse_error(row));
    };
    if weekday.is_none()
        || fields.next().is_some()
        || !(1970..=9999).contains(&year)
        || day == 0
        || day > days_in_month(year, month)
    {
        return Err(parse_error(row));
    }

    Ok(u64::from(year) * 10_000_000_000
        + u64::from(month) * 100_000_000
        + u64::from(day) * 1_000_000
        + u64::from(hour) * 10_000
        + u64::from(minute) * 100
        + u64::from(second))
}

fn parse_month(value: &str) -> Option<u32> {
    match value {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn parse_time(value: &str) -> Option<(u32, u32, u32)> {
    let mut fields = value.split(':');
    let hour = fields.next()?.parse::<u32>().ok()?;
    let minute = fields.next()?.parse::<u32>().ok()?;
    let second = fields.next()?.parse::<u32>().ok()?;
    if fields.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((hour, minute, second))
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn parse_error(row: &str) -> crate::error::PijError {
    crate::error::PijError::Adapter {
        adapter: "process-liveness/ps".to_string(),
        message: format!(
            "could not parse `ps -o lstart=` row {row:?}; expected C-locale `Www Mmm DD HH:MM:SS YYYY`"
        ),
    }
}

/// Everything the registry knows about one seat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatDescriptor {
    /// Stable id.
    pub id: SeatId,
    /// Machine alias in a federated VIEW. Registry rows keep this absent at rest;
    /// the serving daemon stamps its current alias so an operator rename cannot
    /// leave every stored seat carrying stale machine identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Read-projection badge retained when decoding a federated roster. Raw
    /// descriptors never serialize it; the roster/card boundary owns emission.
    #[serde(default, skip_serializing)]
    pub badge: Option<String>,
    /// Read-projection event freshness, in epoch milliseconds. Not registry state.
    #[serde(default, skip_serializing)]
    pub last_event_at: Option<u64>,
    /// Harness it runs.
    pub harness: Harness,
    /// Harness-native session id, when the registering runtime supplied one.
    /// Serialized as `session` on seat projections; `None` is an explicit
    /// unknown, not a generated identity.
    #[serde(default, rename = "session")]
    pub harness_session: Option<String>,
    /// Loaded pij extension build; `None` means no build identity was reported.
    #[serde(default)]
    pub extension_build: Option<String>,
    /// Real directory the registering runtime loaded its pij extension from.
    #[serde(default)]
    pub extension_path: Option<String>,
    /// tmux pane, when it has one. `None` means paneless (external pull mode) —
    /// distinct from "we have not looked", which is the absence of the seat.
    pub pane: Option<String>,
    /// Process identity, when the seat has been bound to a live process.
    pub proc: Option<ProcIdentity>,
    /// Absolute path of the folder the seat works in.
    pub folder: String,
    /// Observed state.
    pub state: SystemState,
    /// Declared state, when the seat has declared one.
    pub semantic_state: Option<SemanticState>,
    /// Role, when assigned.
    pub role: Option<String>,
    /// The seat that governs this one, self-declared at adoption.
    pub parent: Option<SeatId>,
    /// A relay/bridge forwards its inbox to an external sink; its idleness is
    /// correct by design and it is never watched.
    #[serde(default)]
    pub relay: bool,
    /// When the seat was tombstoned, if it was. A live seat has `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstoned_at: Option<u64>,
    /// Why it was tombstoned. The row IS the post-mortem, so the reason has to be
    /// readable through the port — before this, only the fake could answer, which
    /// meant the promise could not be proven against SQL (u-store D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_reason: Option<String>,

    /// Whether this seat was launched configured to ACCEPT cross-session inbound
    /// messages — the precondition for socket delivery.
    ///
    /// `None` means UNKNOWN, and unknown is treated as closed. This is stamped at
    /// spawn by whoever passed the setting; nothing infers it, and nothing reads
    /// the harness's own configuration to guess it.
    ///
    /// It exists because u-uds MEASURED what the design document asserted
    /// (Claude 2.1.251, isolated seat, wave 3): an authenticated frame to a seat
    /// without this setting is HELD behind a five-minute approval dialog — "sender
    /// did not attest its permission mode" — and the socket returns NOTHING. No
    /// drop report, no positive ack, zero bytes over two seconds. The negative-ack
    /// timer the fork uses relies on a sender SESSION having a channel to be
    /// silent on; a daemon has no such channel, so silence stopped being evidence
    /// while still producing the same confident word.
    ///
    /// So a socket transport must know this BEFORE it writes. Without it we would
    /// be writing bytes and then reasoning about what they became, which is how
    /// "delivered" comes to mean "held in a dialog nobody clicked".
    ///
    /// **The hold is not a bug to route around; it is Claude's security model
    /// working.** Corroborated independently by the prime's pre-port PoC, which
    /// hit the same wall from the other side: a foreign non-Claude sender CAN
    /// write the socket, and the attestation is derived from THE SENDER'S VERIFIED
    /// PID BEING A CLAUDE SESSION — never from a wire field. That is
    /// anti-permission-laundering by design, and it means a daemon can never
    /// truthfully attest. The only honest opens are this recipient-side setting,
    /// stamped at spawn, or Claude-side changes we do not control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_session_inbound_accept: Option<bool>,
    /// The current Copilot process/native session attested extension-owned inbox delivery.
    /// False for legacy rows, pre-bind spawns, and retired or replaced incarnations.
    #[serde(default)]
    pub native_extension_delivery: bool,

    // --- bind evidence (R5 items 13-15) ------------------------------------
    //
    // The class these four close: **the registry row lacked facts another
    // surface already held.** `pij whoami` knew a seat's folder while its
    // descriptor read null; a spawn's argv and the pane footer both knew the
    // model while the row did not. A fact that exists in one instrument and not
    // in the authority is a fact nobody can act on.
    //
    // All optional, because a seat adopted by a human never had a spawn id and
    // pretending otherwise would be the same lie in the other direction.
    /// The launch this seat came from, self-declared at registration. `None` for
    /// a seat that was adopted rather than spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_id: Option<String>,
    /// The model the seat is actually running, as the spawn requested it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider behind that model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Thinking effort, where the runtime has levels at all. Absent is a valid
    /// "this model has no levels" — the same fold u-models inherited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// A seat's status card: what it last said it did, and what it said it would do.
///
/// Shared between `u-report` (which writes them) and `u-anomalies` (whose
/// staleness detector reads them), so it lives here rather than in either.
///
/// **The limit is a documented constant, not a silent truncation.** TS clipped a
/// card at 280 characters with no error and no note, so the end of a card
/// vanished into a length nobody had been told about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Whose card this is.
    pub seat: SeatId,
    /// What the seat just finished.
    pub did: String,
    /// What it intends to do next.
    pub next: String,
    /// When it was written: milliseconds since the Unix epoch, from the caller's
    /// clock — core reads no clock of its own.
    pub at: u64,
    /// The sequence the store assigned, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<Seq>,
}

/// The documented maximum length of a card field, after whitespace collapsing.
///
/// One const, named, with the error that cites it living beside
/// [`crate::error::PijError::ReportTooLong`]. A limit nobody can name is a limit
/// that eats the end of somebody's sentence.
pub const CARD_LIMIT: usize = 280;

/// The documented maximum length of a state NOTE, after whitespace collapsing.
///
/// A DIFFERENT limit from [`CARD_LIMIT`], and deliberately so: the TypeScript
/// generation callers use today caps `report blocked`/`report question` text at
/// 200 (`REPORT_NOTE_MAX_LENGTH`, `.pi/extensions/pij/core/cli.ts:816`) while
/// capping did/next at 280. Naming it here rather than inlining 200 at the one
/// boundary that enforces it keeps the two limits distinguishable — a handler
/// that reused `CARD_LIMIT` for notes would accept a 201-character note and look
/// correct in every test that only exercised 280.
pub const NOTE_LIMIT: usize = 200;

impl SeatDescriptor {
    /// A minimal descriptor: unbound, idle, undeclared.
    pub fn new(id: impl Into<SeatId>, harness: Harness, folder: impl Into<String>) -> Self {
        SeatDescriptor {
            id: id.into(),
            machine: None,
            badge: None,
            last_event_at: None,
            harness,
            harness_session: None,
            extension_build: None,
            extension_path: None,
            pane: None,
            proc: None,
            folder: folder.into(),
            state: SystemState::Idle,
            semantic_state: None,
            role: None,
            parent: None,
            relay: false,
            tombstoned_at: None,
            tombstone_reason: None,
            cross_session_inbound_accept: None,
            native_extension_delivery: false,
            spawn_id: None,
            model: None,
            provider: None,
            effort: None,
        }
    }

    /// Is this seat eligible for a watchdog nudge right now?
    ///
    /// Three independent reasons to leave a seat alone, each a real incident:
    /// a relay's silence is its job; a declared non-working state is deliberate;
    /// a working seat is not stalled just because it is quiet.
    pub fn nudgeable(&self) -> bool {
        !self.relay
            && self.state != SystemState::Working
            && self.semantic_state.is_none_or(SemanticState::nudgeable)
    }
}

impl From<String> for SeatId {
    fn from(value: String) -> Self {
        SeatId(value)
    }
}

/// Where a message is going: a seat, and optionally the machine it lives on.
///
/// **`machine: None` always means LOCAL** (PRD req-0017). That invariant is what
/// keeps single-machine usage unchanged when federation lands, so it is asserted
/// as its own test rather than left as a comment.
///
/// Parsed by the CLI from `<seat>` or `<seat>@<machine-alias>`; consumed by the
/// daemon already resolved. Landed by the PM before either unit could invent its
/// own half.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    /// The seat.
    pub seat: SeatId,
    /// The machine alias, or `None` for this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl Destination {
    /// A destination on this machine.
    pub fn local(seat: impl Into<SeatId>) -> Self {
        Destination {
            seat: seat.into(),
            machine: None,
        }
    }

    /// Is this destination on the machine handling it?
    pub fn is_local(&self) -> bool {
        self.machine.is_none()
    }
}

/// A monotonic sequence number handed out by the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);

/// Why a command refused, in a form a caller can BRANCH on.
///
/// `meta` is prose for a human; an exit-code table cannot be built from prose.
/// u-cli found this: "refused" and "not found" were indistinguishable to a
/// client, so any four-row exit contract would have been guesswork over string
/// matching.
///
/// **HTTP status maps FROM this, never the reverse.** The daemon's own word is
/// the truth; the status code is a courtesy to `curl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The daemon understood and declined — a tombstoned recipient, a refused
    /// registration, a capability the caller does not hold.
    Refused,
    /// The thing asked about does not exist. Distinct from `Refused`: absent is
    /// not the same as forbidden, and a caller does different things about each.
    NotFound,
    /// Missing or wrong bearer key.
    Auth,
    /// An envelope version this build will not act on. Never half-read.
    Skew,
    /// A requested cursor is beyond the spine that must serve it: that history
    /// was reset. NON-TRANSIENT and the caller's own cursor is at fault, so a
    /// consumer must resume from the source's real position rather than retry.
    ///
    /// It is a KIND rather than a message because the recovery has to branch on
    /// it: the worker keyed on the refusal's prose, so any wording change would
    /// have silently disabled the recovery and returned the system to the exact
    /// silence it was built to end (review F10). This project ruled in wave 3
    /// that nothing may branch on prose, and then branched on prose.
    CursorReset,
    /// An adapter failed at its boundary, carrying its own words in `meta`.
    Adapter,
}

/// The wire envelope every command answers in.
///
/// `data` is open by contract — a verb's payload is whatever that verb returns —
/// and `v` is what lets a reader refuse a payload from a future it does not
/// understand instead of half-reading it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// Did the command succeed.
    pub ok: bool,
    /// The command that produced this envelope.
    pub command: String,
    /// Envelope version.
    pub v: u32,
    /// The payload, absent on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Out-of-band notes: timing, warnings, the next action to take. PROSE, for a
    /// human — never the thing a client branches on. See `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<String>,
    /// Why it failed, in a form a caller can branch on. Absent when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorKind>,
    /// Command-specific machine-readable failure facts. Absent on success and
    /// when a refusal has no structured evidence beyond its error category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Original validated client response, excluded from the wire representation.
    /// JSON clients retain its ordering, whitespace, and unknown fields. A newly
    /// synthesized response must not inherit a source it no longer represents.
    #[serde(skip)]
    pub raw_json: Option<String>,
}

impl<T> Envelope<T> {
    /// A successful envelope for `command`.
    pub fn ok(command: impl Into<String>, data: T) -> Self {
        Envelope {
            ok: true,
            command: command.into(),
            v: ENVELOPE_VERSION,
            data: Some(data),
            meta: None,
            error: None,
            details: None,
            raw_json: None,
        }
    }

    /// A failed envelope for `command`, carrying the reason in `meta`.
    pub fn err(command: impl Into<String>, meta: impl Into<String>) -> Self {
        Envelope {
            ok: false,
            command: command.into(),
            v: ENVELOPE_VERSION,
            data: None,
            meta: Some(meta.into()),
            error: None,
            details: None,
            raw_json: None,
        }
    }

    /// A failed envelope that says WHY in a form a caller can branch on.
    pub fn refused(command: impl Into<String>, error: ErrorKind, meta: impl Into<String>) -> Self {
        Envelope {
            ok: false,
            command: command.into(),
            v: ENVELOPE_VERSION,
            data: None,
            meta: Some(meta.into()),
            error: Some(error),
            details: None,
            raw_json: None,
        }
    }
}

/// The envelope version this build speaks.
pub const ENVELOPE_VERSION: u32 = 2;

/// A message in flight between two seats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Msg {
    /// Sender.
    pub from: SeatId,
    /// Recipient.
    pub to: SeatId,
    /// Body.
    pub body: String,
    /// Stable id, so a route can be traced end to end. Every delivery logs it.
    pub msg_id: String,
    /// The machine the sender is on, when it is not this one.
    ///
    /// **R7-AMEND-1.** `None` means local, symmetric with `Destination.machine`.
    ///
    /// u-federation found that a forwarded message arrived with no way to answer
    /// it: `from` is a bare `SeatId`, so an ordinary reply routed locally — and if
    /// both machines held a seat with the same name, the receiving daemon's own
    /// self-address refusal fired on a message from a stranger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_machine: Option<String>,
    /// The message this one ANSWERS, when it is a reply.
    ///
    /// Durable, because correlation that lives only in a client's memory is not
    /// correlation: the wire contract promises a message and its answer can be
    /// joined "without guessing", and review found the field was accepted at the
    /// route, dropped at the boundary, and absent from the queue payload and both
    /// events — declared, sent, and silently discarded.
    ///
    /// `None` is not a reply. Nothing infers one from timing or adjacency: a turn
    /// that happens to follow a message is not an answer to it, and manufacturing
    /// that link would be the same class of invention as a receipt claiming an
    /// observation nobody made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    /// A REMOTE-CONTROL command (`/compact`, `/new`) rather than a body.
    ///
    /// This exists because the routing decision depends on it and nothing else
    /// carried the fact: Claude renders a socket-delivered `/compact` as plain
    /// TEXT rather than executing it, so a command must take the pty path even
    /// for a seat with a perfect socket. `can_deliver` was widened to take the
    /// message (R3-AMEND-3) to inspect exactly this — and u-uds found that the
    /// amendment had landed the SIGNATURE without landing the FIELD, so the
    /// carve-out it exists for could not be implemented or honestly tested.
    ///
    /// `None` is a body. Producers populate it; nothing infers it from a leading
    /// slash, because a body may legitimately begin with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Where the evidence for a delivery claim came from (erratum-23b, s105).
///
/// **A receipt word names what was OBSERVED, never what was INFERRED.** Where the
/// two differ, the weaker word is the honest one, and a consumer that cannot
/// distinguish these renders the weakest applicable claim.
///
/// The defect this closes, found by s105 in the TS tree: `confirmed` was returned
/// immediately after pressing Enter, having observed nothing at all. The TYPE
/// permitted the lie, so one got written — which is why `origin` is a required
/// field here rather than an optional annotation. "Confirmed without an origin"
/// is now unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryOrigin {
    /// Bytes submitted into a tmux pane — and nothing more. The pane accepted
    /// the write, but no recipient process acknowledged or decoded it.
    TypedToPane,
    /// Bytes accepted by a socket or RPC endpoint — and nothing more. The
    /// recipient process observed the write, but did not acknowledge the message.
    InjectedToTransport,
    /// A positive transport-level acknowledgement came back: Copilot RPC returns
    /// a message id, the Telegram bridge confirms.
    VerifiedArrival,
    /// An inbox client received and decoded the message, then explicitly
    /// acknowledged its job id. Over HTTP, bearer auth attests this only to the
    /// machine (`local`, a configured peer alias, or `unknown-peer`), not to an
    /// individual seat; the append-only inbox-ack audit event records that
    /// evidence grade. It is never minted when the GET request is merely issued.
    ReaderRead,
}

impl DeliveryOrigin {
    /// How strong this evidence is, weakest first. Used when several claims
    /// describe one delivery and a consumer must render only one.
    pub const fn strength(self) -> u8 {
        match self {
            DeliveryOrigin::TypedToPane => 0,
            DeliveryOrigin::InjectedToTransport => 1,
            DeliveryOrigin::VerifiedArrival => 2,
            DeliveryOrigin::ReaderRead => 3,
        }
    }

    /// The honest claim when two observations disagree: the WEAKER one.
    pub fn weakest(self, other: Self) -> Self {
        if other.strength() < self.strength() {
            other
        } else {
            self
        }
    }
}

/// Terminal inbox recovery outcomes; none is evidence that a body was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryFailure {
    /// Three extension delivery leases expired without consumption.
    #[serde(rename = "undelivered:lease-exhausted")]
    LeaseExhausted,
    /// The runtime swallowed the bounded resend attempts.
    #[serde(rename = "undelivered:harness-swallowed")]
    HarnessSwallowed,
    /// An authorized operator released a running head with evidence.
    #[serde(rename = "undelivered:operator-released")]
    OperatorReleased,
    /// The native receiver stopped renewing its lease, regardless of host liveness.
    #[serde(rename = "undelivered:native-receiver-unavailable")]
    NativeReceiverUnavailable,
}

impl DeliveryFailure {
    /// Stable persisted and wire vocabulary.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeaseExhausted => "undelivered:lease-exhausted",
            Self::HarnessSwallowed => "undelivered:harness-swallowed",
            Self::OperatorReleased => "undelivered:operator-released",
            Self::NativeReceiverUnavailable => "undelivered:native-receiver-unavailable",
        }
    }
}

/// What honestly happened to a message.
///
/// `Queued` is a promise the store has already kept — the row exists — not an
/// optimistic guess. TS defect #1 reported success for messages that vanished
/// because the seat had not bound yet; the distinction between these variants is
/// the fix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "outcome")]
pub enum DeliveryOutcome {
    /// The message reached the seat — qualified by WHAT WAS OBSERVED.
    ///
    /// The origin is required (erratum-23b, ruled by the prime as the v1 wire
    /// vocabulary): a bare "delivered" conflates "we wrote bytes to a transport"
    /// with "the recipient got it", and those are different facts with different
    /// consequences for whoever is waiting.
    Delivered {
        /// What was actually observed.
        origin: DeliveryOrigin,
    },
    /// Durably queued; will be delivered when the seat can receive.
    Queued {
        /// Why immediate delivery was deferred, when the deciding layer knows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Persisted queue eligibility (`jobs.not_before`), in Unix milliseconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_retry_at: Option<u64>,
        /// First 12 hex characters of SHA-256 over the recognized draft text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft_sha: Option<String>,
    },
    /// Withheld pending an approval step. PENDING, not final — a held message may
    /// still become delivered, or refused.
    Held {
        /// Why it is being held.
        reason: String,
    },
    /// The recipient REFUSED it. Terminal: the message will not be delivered and
    /// must never be retried.
    ///
    /// Added in plan 110 rather than spelling a denial as `Held`. Claude's socket
    /// reports `status:"denied"` when the recipient operator declines an inbound
    /// message — "it was not delivered to their Claude session", in the CLI's own
    /// words. Calling that `Held` would be the least-wrong SPELLING of a wrong
    /// CLAIM: `Held` means pending, so anything that retries on it re-prompts a
    /// human who has already said no. A refusal is an answer, and it is given
    /// once.
    Refused {
        /// Why it was refused, in the refusing layer's own words.
        reason: String,
    },
}

/// What a sender is told, and what an auditor can follow.
///
/// The outcome alone is not a receipt: TS could report success for a message that
/// vanished and leave nothing to trace, because no id travelled with the verdict
/// (ermine handover gap #8). A receipt binds the two, so "it said delivered" and
/// "which message" are the same fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The message this verdict is about.
    pub msg_id: String,
    /// What honestly happened to it.
    pub outcome: DeliveryOutcome,
    /// When, in milliseconds since the Unix epoch, from the caller's clock.
    pub at: u64,
    /// The cold-wake guard's verdict for this recipient (plan 157 phase 2):
    /// `clear`, `busy`, `forced` or `unknown: <why>`. Absent where the guard
    /// does not run (controls, FYIs, forwards).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_check: Option<String>,
    /// A caution about how the message was sent (plan 159): a held FYI whose
    /// body looks like a question. Never changes what happened to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A unit of deferred work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// What kind of work.
    pub kind: String,
    /// Work claimed one-at-a-time per key, so two workers never act on one
    /// entity concurrently.
    pub serial_key: String,
    /// Opaque payload for the consumer.
    pub payload: String,
    /// Jobs equal on this key collapse to one live row: N rapid submits produce
    /// one unit of work, not N.
    pub dedupe_key: String,
    /// The paired machine a forwarded message came from (plan 164 review F02).
    /// Jobs collapse on `(kind, dedupe_origin, dedupe_key)`, origin in its own
    /// column, so a peer's id can never collide with a local one. `None` for
    /// local work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_origin: Option<String>,
    /// How many times this job has been retried. **Read-only to callers**: the
    /// queue is the one writer (R4-AMEND-1), and a worker needs to READ it to
    /// compute backoff that survives a restart.
    ///
    /// u-federation found the gap: `retry` wrote the counter and `claim` handed
    /// back a `Job` that could not see it, so the only way to compute an
    /// exponential delay was a process-local tally — which resets on reboot and
    /// leaves the durable field consumed by nobody.
    #[serde(default)]
    pub attempt: u32,
}

/// Durable diagnostic history on a delivery job, not delivery or retry policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryDeferral {
    /// Queue authority identity, preserved across every retry.
    pub job_id: JobId,
    /// Sender's stable message identity.
    pub msg_id: String,
    /// Most recent reason delivery could not proceed.
    pub reason: String,
    /// All deferred attempts, including attempts suppressed by event sampling.
    pub count: u64,
    /// Unix milliseconds of the first deferred attempt, unchanged on reason changes.
    pub since_ms: u64,
}

/// A job's identity in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub u64);

/// How a claimed job ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "outcome")]
pub enum Outcome {
    /// Finished.
    Done,
    /// Failed; the reason is kept for the anomaly detectors.
    Failed {
        /// Why.
        reason: String,
    },
}

/// An append-only fact about the fleet.
///
/// Carries `v` and `at` from its first commit so the reusable event stream
/// (PRD req-0015, unit `u-events`) lands on this shape without a version bump.
/// The forward-compatible `Unknown` decode — an event kind this build has never
/// heard of must be forwarded, not dropped — lives in `wire.rs` at tk-d162,
/// because it is a decoding rule rather than a fact about an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The store-assigned position in the total order, once it HAS one.
    ///
    /// `None` on the write path — nobody has assigned a sequence to an event that
    /// has not been appended yet — and `Some` on every read path, set from the
    /// store's AUTOINCREMENT. Absent and assigned are different facts, kept
    /// distinct rather than collapsed onto a sentinel `0`.
    ///
    /// It exists because `Spine::tail(since)` returns `Vec<Event>`: without a
    /// sequence on the event itself, a consumer cannot advance a durable cursor
    /// from what it just read, so replay and live streams cannot be joined
    /// without gaps or duplicates. Two wave-1 coders found that independently
    /// (u-store D5, u-events D1). Fixing it additively here keeps `ports.rs`
    /// frozen — the trait signature never changed.
    ///
    /// **Never on the wire.** R7 fixes the v1 event shape at exactly
    /// `{v, at, kind, seat, payload}`, and a cursor is a property of a STREAM,
    /// not of the fact that happened: putting it in the body would make every
    /// consumer's payload depend on whether the event came from a tail or a
    /// publish. When the NDJSON stream needs resumability it belongs in the
    /// FRAME — a wave-3 design point this deliberately does not pre-empt.
    /// `crates/core/tests/wire_over_corpus.rs` asserts the absence.
    #[serde(skip)]
    pub seq: Option<Seq>,
    /// Event-schema version, so a reader can refuse a payload from a future it
    /// does not understand instead of half-reading it.
    pub v: u32,
    /// When it happened: milliseconds since the Unix epoch, supplied by the
    /// caller's clock so core stays free of time IO.
    pub at: u64,
    /// What happened.
    pub kind: String,
    /// Which seat it happened to, when it was about one.
    pub seat: Option<SeatId>,
    /// Opaque payload.
    pub payload: String,
}

/// A tmux pane, as this workspace models it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    /// Pane id, e.g. `%255`.
    pub id: String,
    /// Session name.
    pub session: String,
    /// Window name.
    pub window: String,
    /// Pane title.
    pub title: String,
    /// Zero-based cursor column, when tmux supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_x: Option<u32>,
    /// Zero-based cursor row, when tmux supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_y: Option<u32>,
}

/// The process a tmux pane is running, as the daemon DERIVES it from tmux.
///
/// Deliberately NOT a field a caller supplies. Adoption's identity facts have to
/// be observed by the daemon itself: a caller-asserted pid is a claim, and this
/// platform has already paid for treating an asserted identity as a derived one
/// (an unvalidated `PIJ_SESSION_ID` minting a phantom seat). The pair that
/// matters is `(pid, proc_start)` — the pid alone is recycled at boot — and the
/// start time is read from the liveness port, never from tmux, so the two facts
/// come from two different observers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneProcess {
    /// `#{pane_pid}` — the pane's foreground process, which is exactly the pid
    /// the TS CLI records at adopt time.
    pub pid: u32,
    /// `#{pane_current_path}` — the folder the seat is working in.
    pub cwd: String,
}

/// Whether a harness session is ready to receive a turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "readiness")]
pub enum Readiness {
    /// Anchor observed: the seat can take a turn.
    Ready,
    /// The harness is up but busy.
    Busy,
    /// No anchor yet — booting, or never will.
    NotYet {
        /// What was observed instead, so a never-bind is diagnosable rather than
        /// a timeout (the codex never-bind class).
        observed: String,
    },
}

/// The health of a seat's binding to a live process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "bind")]
pub enum BindHealth {
    /// Bound to a live process.
    Bound {
        /// The process it is bound to.
        proc: ProcIdentity,
    },
    /// Not bound, with the evidence for why.
    Unbound {
        /// What was observed.
        evidence: String,
    },
}

/// One row of a harness's model catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRow {
    /// The runtime that executes the model.
    pub runtime: String,
    /// The provider that serves it.
    pub provider: String,
    /// How the user selects it.
    pub selector: String,
    /// What the harness sends on the wire.
    pub request_model_id: String,
    /// Available thinking efforts; **empty means this model has no levels**.
    ///
    /// A `Vec`, not an `Option<Vec>`, and that is a ruling rather than taste: the
    /// TS CLI expresses "no levels" as `"levels": []` on some rows and by OMITTING
    /// the key on others (the claude family), and the raw `null` some upstream
    /// providers send never reaches pij's output. Three encodings of one fact
    /// collapse here into one representation, so no consumer can branch on a
    /// distinction that does not exist. Measured 2026-08-28; services.dd.md
    /// corrected the same day.
    #[serde(default)]
    pub thinking_levels: Vec<String>,
}

/// A liveness verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "liveness")]
pub enum Liveness {
    /// The recorded process is still the running one.
    Active,
    /// The process is gone.
    Dead {
        /// What proved it.
        evidence: String,
    },
    /// The pid exists but belongs to a DIFFERENT process — the recycled-pid
    /// case, which must never be reported as `Active`.
    Recycled {
        /// The start time observed now.
        observed_start: u64,
        /// The start time recorded at bind.
        recorded_start: u64,
    },
}

#[cfg(test)]
mod process_start_tests {
    use super::parse_process_start;
    use crate::error::PijError;

    #[test]
    fn valid_dates_preserve_the_packed_identity() {
        for (row, expected) in [
            ("Sat Aug 29 09:11:48 2026", 20_260_829_091_148),
            ("Thu Jan 01 00:00:00 1970", 19_700_101_000_000),
            ("Thu Feb 29 23:59:59 2024", 20_240_229_235_959),
            ("Tue Feb 29 01:02:03 2000", 20_000_229_010_203),
            ("Fri Dec 31 23:59:59 9999", 99_991_231_235_959),
        ] {
            assert_eq!(parse_process_start(row).unwrap(), expected, "{row}");
        }
    }

    #[test]
    fn whitespace_and_existing_field_flexibility_are_preserved() {
        for row in [
            "Thu Jan  8 00:20:51 2026",
            " \tThu\tJan  8\t00:20:51 2026\n",
            "weekday Jan 8 0:20:51 2026",
        ] {
            assert_eq!(parse_process_start(row).unwrap(), 20_260_108_002_051);
        }
    }

    #[test]
    fn malformed_dates_times_and_rows_keep_the_adapter_error() {
        for row in [
            "",
            "Jan 08 00:20:51 2026",
            "Thu Jan 08 00:20:51 2026 UTC",
            "Thu JAN 08 00:20:51 2026",
            "Thu Jan 00 00:20:51 2026",
            "Thu Apr 31 00:20:51 2026",
            "Thu Feb 29 00:20:51 2026",
            "Thu Feb 29 00:20:51 2100",
            "Thu Jan 08 24:00:00 2026",
            "Thu Jan 08 00:60:00 2026",
            "Thu Jan 08 00:00:60 2026",
            "Thu Jan 08 00:20 2026",
            "Thu Jan 08 00:20:51:00 2026",
            "Thu Jan 08 xx:20:51 2026",
            "Thu Jan 08 00:20:51 1969",
            "Thu Jan 08 00:20:51 10000",
        ] {
            let PijError::Adapter { adapter, message } = parse_process_start(row).unwrap_err()
            else {
                panic!("expected the existing adapter error for {row:?}");
            };
            assert_eq!(adapter, "process-liveness/ps");
            assert_eq!(
                message,
                format!(
                    "could not parse `ps -o lstart=` row {row:?}; expected C-locale `Www Mmm DD HH:MM:SS YYYY`"
                )
            );
        }
    }

    #[test]
    fn supplied_wall_time_is_encoded_without_timezone_conversion() {
        let record_utc = parse_process_start("Mon Aug 31 10:02:54 2026").unwrap();
        let observed_local = parse_process_start("Mon Aug 31 20:02:54 2026").unwrap();
        assert_eq!(record_utc, 20_260_831_100_254);
        assert_eq!(observed_local, 20_260_831_200_254);
        assert_ne!(record_utc, observed_local);
    }
}
