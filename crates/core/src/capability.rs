//! Seat capabilities: the records the secured issuer keeps (plan 223 AC2,
//! plan 221 workshop 001 contracts a and b).
//!
//! A capability is a per-incarnation secret. Only its SHA-256 digest is ever
//! recorded, and it is issued only by bootstrap evidence (a spawn's single-use
//! nonce, or, from AC3, the socket peer's harness boundary) or by presenting
//! the current capability (rotation). These are the stored shapes and the pure
//! checks every [`crate::ports::Registry`] implementation runs inside its one
//! issuance transaction, so the fake and SQLite answer identically.

use crate::error::PijError;
use crate::model::{Harness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use crate::ports::PutBinding;

/// Bytes in a capability, a nonce and either's digest.
pub const SECRET_LEN: usize = 32;

/// A SHA-256 digest of a 32-byte secret. Plaintext never crosses a port.
pub type Digest = [u8; SECRET_LEN];

/// The refusal for a nonce another registration already consumed.
pub const NONCE_USED_CODE: &str = "E-RS-SPAWN-NONCE-USED";
/// The refusal for a nonce that is unknown, expired or for another seat.
pub const NONCE_INVALID_CODE: &str = "E-RS-SPAWN-NONCE-INVALID";
/// The refusal for a presented capability that is not the seat's current one.
pub const CAPABILITY_CODE: &str = "E-RS-SEAT-CAPABILITY";

/// One spawn's registration nonce, as stored: its digest, never the nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnNonce {
    /// The daemon-minted spawn id the nonce was launched with.
    pub spawn_id: String,
    /// The seat the spawn intends to register.
    pub seat: SeatId,
    /// The harness launched.
    pub harness: Harness,
    /// SHA-256 of the nonce.
    pub digest: Digest,
    /// Epoch milliseconds at issue.
    pub issued_at: u64,
    /// Epoch milliseconds at which the spawn's registration window closes.
    pub expires_at: u64,
}

/// A stored nonce and whether a registration consumed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonceRecord {
    /// The nonce as issued.
    pub nonce: SpawnNonce,
    /// Epoch milliseconds of the registration that consumed it.
    pub consumed_at: Option<u64>,
}

/// How a capability was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityVia {
    /// A spawn's nonce (bootstrap evidence, path a).
    Nonce,
    /// The socket peer's harness boundary (bootstrap evidence, path b, AC3).
    Peer,
    /// The current capability, presented at re-registration (path c).
    Rotation,
}

impl CapabilityVia {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nonce => "nonce",
            Self::Peer => "peer",
            Self::Rotation => "rotation",
        }
    }

    /// Parse a stored spelling; anything else is refused, never defaulted.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "nonce" => Some(Self::Nonce),
            "peer" => Some(Self::Peer),
            "rotation" => Some(Self::Rotation),
            _ => None,
        }
    }
}

/// One live incarnation's capability, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRow {
    /// The seat.
    pub seat: SeatId,
    /// The process that bootstrapped the incarnation and holds the token,
    /// when the daemon observed it (peer credentials, AC3).
    pub owner: Option<ProcIdentity>,
    /// The harness process the capability is bound to: the seat's `proc` at
    /// issue. A seat whose process changes no longer holds it.
    pub boundary: Option<ProcIdentity>,
    /// The harness session at issue.
    pub session: Option<String>,
    /// SHA-256 of the current token.
    pub digest: Digest,
    /// SHA-256 of the token it rotated from, valid until the new one is first
    /// presented (two-phase rotation).
    pub prev_digest: Option<Digest>,
    /// Epoch milliseconds at issue.
    pub issued_at: u64,
    /// How it was issued.
    pub via: CapabilityVia,
}

impl CapabilityRow {
    /// Does `digest` name this row's current or still-valid previous token?
    pub fn holds(&self, digest: &Digest) -> bool {
        self.digest == *digest || self.prev_digest.as_ref() == Some(digest)
    }
}

/// The evidence a registration presents to be issued a capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueProof {
    /// A spawn's nonce: its spawn id and the nonce's digest.
    Nonce {
        /// The spawn id the registration claims.
        spawn_id: String,
        /// SHA-256 of the presented nonce.
        digest: Digest,
    },
    /// The socket peer's nearest harness boundary was verified by the daemon
    /// (path b). The daemon constructs this only from peer credentials (AC3);
    /// the store records it without a stored proof.
    Peer,
    /// The seat's current (or still-valid previous) capability.
    Token {
        /// SHA-256 of the presented capability.
        digest: Digest,
    },
}

impl IssueProof {
    /// How a capability issued on this proof is recorded.
    pub fn via(&self) -> CapabilityVia {
        match self {
            Self::Nonce { .. } => CapabilityVia::Nonce,
            Self::Peer => CapabilityVia::Peer,
            Self::Token { .. } => CapabilityVia::Rotation,
        }
    }
}

/// One issuance: the proof, and the new capability's digest and owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityIssue {
    /// The evidence presented.
    pub proof: IssueProof,
    /// SHA-256 of the newly minted capability.
    pub digest: Digest,
    /// The owning process, when observed.
    pub owner: Option<ProcIdentity>,
    /// Epoch milliseconds now.
    pub now_ms: u64,
}

/// Why an issuance was refused. Nothing of the registration is committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueRefusal {
    /// The nonce matched and another registration already consumed it.
    NonceUsed,
    /// No nonce with this spawn id and digest is stored.
    NonceUnknown,
    /// The nonce's registration window closed.
    NonceExpired,
    /// The nonce was issued for another seat.
    NonceForAnotherSeat,
    /// The presented capability is not the seat's current one.
    TokenInvalid,
    /// The registration binds no process: a capability belongs to an incarnation.
    Unbound,
}

impl IssueRefusal {
    /// The stable machine-readable code.
    pub fn code(self) -> &'static str {
        match self {
            Self::NonceUsed => NONCE_USED_CODE,
            Self::NonceUnknown | Self::NonceExpired | Self::NonceForAnotherSeat => {
                NONCE_INVALID_CODE
            }
            Self::TokenInvalid | Self::Unbound => CAPABILITY_CODE,
        }
    }

    /// The stable detail word, carried as the refusal's record.
    pub fn detail(self) -> &'static str {
        match self {
            Self::NonceUsed => "used",
            Self::NonceUnknown => "unknown",
            Self::NonceExpired => "expired",
            Self::NonceForAnotherSeat => "another-seat",
            Self::TokenInvalid => "not-current",
            Self::Unbound => "unbound",
        }
    }

    /// The inverse of [`Self::code`] and [`Self::detail`].
    pub fn from_parts(code: &str, detail: &str) -> Option<Self> {
        [
            Self::NonceUsed,
            Self::NonceUnknown,
            Self::NonceExpired,
            Self::NonceForAnotherSeat,
            Self::TokenInvalid,
            Self::Unbound,
        ]
        .into_iter()
        .find(|refusal| refusal.code() == code && refusal.detail() == detail)
    }

    /// Carry the refusal through a publication boundary that speaks
    /// [`PijError`]; [`Self::from_error`] reads it back.
    pub fn into_error(self) -> PijError {
        PijError::GovernanceRefused {
            code: self.code().to_string(),
            record: self.detail().to_string(),
        }
    }

    /// The refusal [`Self::into_error`] carried, if `error` is one.
    pub fn from_error(error: &PijError) -> Option<Self> {
        match error {
            PijError::GovernanceRefused { code, record } => Self::from_parts(code, record),
            _ => None,
        }
    }

    /// A human sentence. Never names a secret.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NonceUsed => {
                "this spawn's registration nonce was already consumed by another registration; nothing was written"
            }
            Self::NonceUnknown => {
                "no registration nonce with this spawn id is outstanding; nothing was written (register without spawn_nonce to register with public facts only)"
            }
            Self::NonceExpired => {
                "this spawn's registration nonce expired with its registration window; nothing was written (register without spawn_nonce to register with public facts only)"
            }
            Self::NonceForAnotherSeat => {
                "this registration nonce was issued for another seat; nothing was written"
            }
            Self::TokenInvalid => {
                "the presented seat capability is not this seat's current one; nothing was written (register without it to register with public facts only)"
            }
            Self::Unbound => {
                "a seat capability belongs to a process incarnation: register with pid and proc_start; nothing was written"
            }
        }
    }
}

/// The result of one issuance transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueOutcome {
    /// The seat row and its capability committed together.
    Issued {
        /// The `seat.put` sequence.
        seq: Seq,
        /// The binding the put replaced.
        binding: PutBinding,
    },
    /// Nothing of the registration committed.
    Refused(IssueRefusal),
}

/// Check a nonce presentation against its stored record.
///
/// A digest that does not match reads as unknown, so a spawn id alone learns
/// nothing; a consumed nonce is reported only to its holder.
///
/// # Errors
/// The refusal that applies.
pub fn check_nonce(
    record: Option<&NonceRecord>,
    seat: &SeatId,
    spawn_id: &str,
    digest: &Digest,
    now_ms: u64,
) -> std::result::Result<(), IssueRefusal> {
    let Some(record) =
        record.filter(|record| record.nonce.spawn_id == spawn_id && record.nonce.digest == *digest)
    else {
        return Err(IssueRefusal::NonceUnknown);
    };
    if record.consumed_at.is_some() {
        return Err(IssueRefusal::NonceUsed);
    }
    if record.nonce.expires_at <= now_ms {
        return Err(IssueRefusal::NonceExpired);
    }
    if record.nonce.seat != *seat {
        return Err(IssueRefusal::NonceForAnotherSeat);
    }
    Ok(())
}

/// Check a proof for `seat`, whose registration binds `proc`.
///
/// # Errors
/// The refusal that applies.
pub fn check_proof(
    proof: &IssueProof,
    seat: &SeatId,
    proc: Option<ProcIdentity>,
    nonce: Option<&NonceRecord>,
    current: Option<&CapabilityRow>,
    now_ms: u64,
) -> std::result::Result<(), IssueRefusal> {
    if proc.is_none() {
        return Err(IssueRefusal::Unbound);
    }
    match proof {
        IssueProof::Nonce { spawn_id, digest } => {
            check_nonce(nonce, seat, spawn_id, digest, now_ms)
        }
        IssueProof::Peer => Ok(()),
        IssueProof::Token { digest } => current
            .filter(|row| row.seat == *seat && row.holds(digest))
            .map(|_| ())
            .ok_or(IssueRefusal::TokenInvalid),
    }
}

/// Check a same-incarnation re-registration that presents its capability
/// without asking to rotate it: the seat's own current (or still-valid
/// previous) digest, bound to the very process the registration binds. A new
/// process must rotate explicitly, so nothing is kept across incarnations.
///
/// # Errors
/// The refusal that applies.
pub fn check_held(
    current: Option<&CapabilityRow>,
    seat: &SeatId,
    proc: Option<ProcIdentity>,
    digest: &Digest,
) -> std::result::Result<(), IssueRefusal> {
    if proc.is_none() {
        return Err(IssueRefusal::Unbound);
    }
    current
        .filter(|row| row.seat == *seat && row.boundary == proc && row.holds(digest))
        .map(|_| ())
        .ok_or(IssueRefusal::TokenInvalid)
}

/// The row an accepted issuance stores, bound to the written descriptor's process.
pub fn issued_row(descriptor: &SeatDescriptor, issue: &CapabilityIssue) -> CapabilityRow {
    CapabilityRow {
        seat: descriptor.id.clone(),
        owner: issue.owner,
        boundary: descriptor.proc,
        session: descriptor.harness_session.clone(),
        digest: issue.digest,
        prev_digest: match &issue.proof {
            IssueProof::Token { digest } => Some(*digest),
            IssueProof::Nonce { .. } | IssueProof::Peer => None,
        },
        issued_at: issue.now_ms,
        via: issue.proof.via(),
    }
}

/// Does writing `descriptor` revoke `row`? A tombstone, or any change of the
/// seat's process, ends the incarnation the capability was issued to. Every
/// registry write applies this in the transaction that writes the seat.
pub fn revoked_by(row: &CapabilityRow, descriptor: &SeatDescriptor) -> bool {
    descriptor.tombstoned_at.is_some() || row.boundary != descriptor.proc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seat: &str, expires_at: u64, consumed: bool) -> NonceRecord {
        NonceRecord {
            nonce: SpawnNonce {
                spawn_id: "spawn-1".into(),
                seat: seat.into(),
                harness: Harness::Omp,
                digest: [7; SECRET_LEN],
                issued_at: 1,
                expires_at,
            },
            consumed_at: consumed.then_some(5),
        }
    }

    #[test]
    fn a_nonce_is_checked_digest_first_then_use_then_window_then_seat() {
        let seat: SeatId = "pij-a".into();
        let fresh = record("pij-a", 100, false);
        assert_eq!(
            check_nonce(Some(&fresh), &seat, "spawn-1", &[7; SECRET_LEN], 50),
            Ok(())
        );
        assert_eq!(
            check_nonce(Some(&fresh), &seat, "spawn-1", &[8; SECRET_LEN], 50),
            Err(IssueRefusal::NonceUnknown),
            "a wrong nonce learns nothing about the spawn"
        );
        assert_eq!(
            check_nonce(Some(&fresh), &seat, "spawn-2", &[7; SECRET_LEN], 50),
            Err(IssueRefusal::NonceUnknown)
        );
        assert_eq!(
            check_nonce(None, &seat, "spawn-1", &[7; SECRET_LEN], 50),
            Err(IssueRefusal::NonceUnknown)
        );
        assert_eq!(
            check_nonce(
                Some(&record("pij-a", 100, true)),
                &seat,
                "spawn-1",
                &[7; SECRET_LEN],
                50
            ),
            Err(IssueRefusal::NonceUsed)
        );
        assert_eq!(
            check_nonce(Some(&fresh), &seat, "spawn-1", &[7; SECRET_LEN], 100),
            Err(IssueRefusal::NonceExpired)
        );
        assert_eq!(
            check_nonce(
                Some(&record("pij-b", 100, false)),
                &seat,
                "spawn-1",
                &[7; SECRET_LEN],
                50
            ),
            Err(IssueRefusal::NonceForAnotherSeat)
        );
    }

    #[test]
    fn a_token_proof_accepts_the_current_or_pending_previous_digest_of_its_own_seat() {
        let proc = Some(ProcIdentity {
            pid: 9,
            proc_start: 3,
        });
        let mut seat = SeatDescriptor::new("pij-a", Harness::Omp, "/abs");
        seat.proc = proc;
        let row = issued_row(
            &seat,
            &CapabilityIssue {
                proof: IssueProof::Token { digest: [1; 32] },
                digest: [2; 32],
                owner: None,
                now_ms: 10,
            },
        );
        assert_eq!(row.via, CapabilityVia::Rotation);
        assert_eq!(row.boundary, proc);
        for (digest, expected) in [
            ([2; 32], Ok(())),
            ([1; 32], Ok(())),
            ([3; 32], Err(IssueRefusal::TokenInvalid)),
        ] {
            assert_eq!(
                check_proof(
                    &IssueProof::Token { digest },
                    &seat.id,
                    proc,
                    None,
                    Some(&row),
                    10
                ),
                expected
            );
        }
        assert_eq!(
            check_proof(
                &IssueProof::Token { digest: [2; 32] },
                &"pij-b".into(),
                proc,
                None,
                Some(&row),
                10
            ),
            Err(IssueRefusal::TokenInvalid),
            "another seat's row never proves this one"
        );
        assert_eq!(
            check_proof(&IssueProof::Peer, &seat.id, None, None, None, 10),
            Err(IssueRefusal::Unbound)
        );
    }

    #[test]
    fn a_held_capability_must_be_this_seats_on_this_process() {
        let mut seat = SeatDescriptor::new("pij-a", Harness::Omp, "/abs");
        seat.proc = Some(ProcIdentity {
            pid: 9,
            proc_start: 3,
        });
        let row = issued_row(
            &seat,
            &CapabilityIssue {
                proof: IssueProof::Token { digest: [1; 32] },
                digest: [2; 32],
                owner: None,
                now_ms: 10,
            },
        );
        let other = Some(ProcIdentity {
            pid: 10,
            proc_start: 4,
        });
        assert_eq!(
            check_held(Some(&row), &seat.id, seat.proc, &[2; 32]),
            Ok(())
        );
        assert_eq!(
            check_held(Some(&row), &seat.id, seat.proc, &[1; 32]),
            Ok(())
        );
        for (proc, digest, expected) in [
            (other, [2; 32], IssueRefusal::TokenInvalid),
            (seat.proc, [3; 32], IssueRefusal::TokenInvalid),
            (None, [2; 32], IssueRefusal::Unbound),
        ] {
            assert_eq!(
                check_held(Some(&row), &seat.id, proc, &digest),
                Err(expected)
            );
        }
    }

    #[test]
    fn a_tombstone_or_a_new_process_revokes() {
        let mut seat = SeatDescriptor::new("pij-a", Harness::Omp, "/abs");
        seat.proc = Some(ProcIdentity {
            pid: 9,
            proc_start: 3,
        });
        let row = issued_row(
            &seat,
            &CapabilityIssue {
                proof: IssueProof::Peer,
                digest: [2; 32],
                owner: None,
                now_ms: 10,
            },
        );
        assert!(!revoked_by(&row, &seat));
        let mut moved = seat.clone();
        moved.proc = Some(ProcIdentity {
            pid: 10,
            proc_start: 4,
        });
        assert!(revoked_by(&row, &moved));
        let mut retired = seat;
        retired.tombstoned_at = Some(1);
        assert!(revoked_by(&row, &retired));
    }

    #[test]
    fn refusals_round_trip_through_code_and_detail() {
        for refusal in [
            IssueRefusal::NonceUsed,
            IssueRefusal::NonceUnknown,
            IssueRefusal::NonceExpired,
            IssueRefusal::NonceForAnotherSeat,
            IssueRefusal::TokenInvalid,
            IssueRefusal::Unbound,
        ] {
            assert_eq!(
                IssueRefusal::from_error(&refusal.into_error()),
                Some(refusal)
            );
        }
        for via in [
            CapabilityVia::Nonce,
            CapabilityVia::Peer,
            CapabilityVia::Rotation,
        ] {
            assert_eq!(CapabilityVia::parse(via.as_str()), Some(via));
        }
    }
}
