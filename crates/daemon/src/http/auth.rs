use std::fmt;
use std::sync::Arc;

/// Accepted inbound bearer credentials, each bound to the machine it names.
///
/// The local key is the daemon's per-boot credential. Each peer key is the
/// persistent pre-shared key for one machine pair, and it authenticates AS that
/// machine's alias: every inbound call is attributed to a named machine, and
/// removing a pairing (then restarting) revokes exactly that machine.
#[derive(Clone)]
pub struct AuthRing {
    local: Arc<str>,
    peers: Arc<[(Arc<str>, String)]>,
}

/// Machine-granularity identity established by a bearer credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AuthenticatedMachine {
    /// The per-boot local key: a client on this machine.
    Local,
    /// A paired peer machine, by the alias this daemon configured for it.
    Peer(Arc<str>),
}

impl AuthenticatedMachine {
    /// `local`, or the peer's alias: what an audit names as the caller.
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Peer(alias) => alias,
        }
    }
}

/// A pairing this daemon refuses to start with. Names aliases, never keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthRingError {
    /// A peer was configured with an empty key.
    EmptyKey(String),
    /// Two peers share one key, so a call could not be attributed.
    DuplicateKey {
        /// The first alias holding the key.
        first: String,
        /// The second alias holding the same key.
        second: String,
    },
    /// A peer key equals this daemon's local key.
    LocalKey(String),
}

impl fmt::Display for AuthRingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey(alias) => write!(formatter, "peer `{alias}` has an empty key"),
            Self::DuplicateKey { first, second } => write!(
                formatter,
                "peers `{first}` and `{second}` share one key; every machine pair needs its own key, or a call cannot be attributed"
            ),
            Self::LocalKey(alias) => write!(
                formatter,
                "peer `{alias}`'s key equals this daemon's local key; a peer key must never grant local access"
            ),
        }
    }
}

impl std::error::Error for AuthRingError {}

/// Compare two secrets without leaking their common prefix length through timing.
///
/// Length is not a secret here (it is fixed by how keys are minted) but the
/// CONTENT is, so the loop always runs over the longer of the two and folds every
/// byte into one accumulator rather than returning at the first difference.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut difference = (left.len() ^ right.len()) as u8;
    let width = left.len().max(right.len());
    for index in 0..width {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= a ^ b;
    }
    difference == 0
}

impl AuthRing {
    /// A ring accepting only the local key: no machine is paired.
    pub fn local(local_key: String) -> Self {
        Self {
            local: local_key.into(),
            peers: Arc::from([]),
        }
    }

    /// Build the ring from the local key and `(alias, key)` pairings.
    ///
    /// # Errors
    /// An empty peer key, two peers sharing a key, or a peer key equal to the
    /// local key. Each would make a call unattributable or over-privileged.
    pub fn new(
        local_key: String,
        peers: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, AuthRingError> {
        let mut accepted: Vec<(Arc<str>, String)> = Vec::new();
        for (alias, key) in peers {
            if key.is_empty() {
                return Err(AuthRingError::EmptyKey(alias));
            }
            if constant_time_eq(&key, &local_key) {
                return Err(AuthRingError::LocalKey(alias));
            }
            if let Some((first, _)) = accepted
                .iter()
                .find(|(_, held)| constant_time_eq(held, &key))
            {
                return Err(AuthRingError::DuplicateKey {
                    first: first.to_string(),
                    second: alias,
                });
            }
            accepted.push((alias.into(), key));
        }
        Ok(Self {
            local: local_key.into(),
            peers: accepted.into(),
        })
    }

    pub(crate) fn authenticate(&self, authorization: &str) -> Option<AuthenticatedMachine> {
        let token = authorization.strip_prefix("Bearer ")?;
        // Constant-time per candidate, and EVERY candidate is compared: no early
        // exit on the first match either, so acceptance time does not reveal a
        // key's position in the ring. Keys are unique by construction, so at
        // most one candidate matches.
        let local = constant_time_eq(&self.local, token);
        let mut peer = None;
        for (alias, key) in self.peers.iter() {
            if constant_time_eq(key, token) {
                peer = Some(alias);
            }
        }
        if local {
            Some(AuthenticatedMachine::Local)
        } else {
            peer.map(|alias| AuthenticatedMachine::Peer(Arc::clone(alias)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthRing, AuthRingError, AuthenticatedMachine};

    fn pair(alias: &str, key: &str) -> (String, String) {
        (alias.to_string(), key.to_string())
    }

    #[test]
    fn every_call_is_attributed_to_a_named_machine() {
        let ring = AuthRing::new(
            "local-secret".to_string(),
            [
                pair("laptop", "laptop-secret"),
                pair("desktop", "desktop-secret"),
            ],
        )
        .expect("ring");
        assert_eq!(
            ring.authenticate("Bearer local-secret"),
            Some(AuthenticatedMachine::Local)
        );
        assert_eq!(
            ring.authenticate("Bearer laptop-secret"),
            Some(AuthenticatedMachine::Peer("laptop".into()))
        );
        assert_eq!(
            ring.authenticate("Bearer desktop-secret"),
            Some(AuthenticatedMachine::Peer("desktop".into()))
        );
        assert_eq!(ring.authenticate("Bearer wrong"), None);
        assert_eq!(ring.authenticate("laptop-secret"), None, "scheme required");
    }

    #[test]
    fn removing_a_pairing_revokes_exactly_that_machine() {
        let before = AuthRing::new(
            "local".to_string(),
            [pair("laptop", "laptop-key"), pair("desktop", "desktop-key")],
        )
        .expect("ring");
        assert!(before.authenticate("Bearer laptop-key").is_some());
        let after =
            AuthRing::new("local".to_string(), [pair("desktop", "desktop-key")]).expect("ring");
        assert_eq!(after.authenticate("Bearer laptop-key"), None);
        assert_eq!(
            after.authenticate("Bearer desktop-key"),
            Some(AuthenticatedMachine::Peer("desktop".into()))
        );
    }

    #[test]
    fn a_ring_that_cannot_attribute_or_would_over_grant_is_refused() {
        assert_eq!(
            AuthRing::new(
                "local".to_string(),
                [pair("laptop", "same"), pair("desktop", "same")]
            )
            .err(),
            Some(AuthRingError::DuplicateKey {
                first: "laptop".to_string(),
                second: "desktop".to_string()
            })
        );
        assert_eq!(
            AuthRing::new("local".to_string(), [pair("laptop", "local")]).err(),
            Some(AuthRingError::LocalKey("laptop".to_string()))
        );
        assert_eq!(
            AuthRing::new("local".to_string(), [pair("laptop", "")]).err(),
            Some(AuthRingError::EmptyKey("laptop".to_string()))
        );
        let refused = AuthRing::new(
            "local".to_string(),
            [
                pair("laptop", "secret-bytes"),
                pair("desktop", "secret-bytes"),
            ],
        )
        .err()
        .expect("refused")
        .to_string();
        assert!(!refused.contains("secret-bytes"), "{refused}");
    }
}
