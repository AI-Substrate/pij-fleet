use std::sync::Arc;

/// Accepted inbound bearer credentials.
///
/// The first credential is the daemon's per-boot local key. The remaining keys
/// are persistent per-machine-pair credentials established by manual bootstrap.
/// Removing a peer key from this ring is how an operator revokes that machine.
#[derive(Clone)]
pub struct AuthRing {
    tokens: Arc<[String]>,
}

/// Machine-granularity identity established by a bearer credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthenticatedMachine(pub(crate) &'static str);

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
    /// Build the ring from one local key and configured peer keys.
    pub fn new(local_key: String, peer_keys: impl IntoIterator<Item = String>) -> Self {
        let tokens: Vec<String> = std::iter::once(local_key).chain(peer_keys).collect();
        Self {
            tokens: tokens.into(),
        }
    }

    pub(crate) fn authenticate(&self, authorization: &str) -> Option<AuthenticatedMachine> {
        let token = authorization.strip_prefix("Bearer ")?;
        // Constant-time per candidate, and EVERY candidate is compared: no early
        // exit on the first match either, so acceptance time does not reveal a
        // key's position in the ring. The current ring preserves no peer alias
        // beside a key, so it honestly grades non-local credentials as
        // `unknown-peer` rather than inventing a machine identity.
        let mut local = false;
        let mut peer = false;
        for (index, accepted) in self.tokens.iter().enumerate() {
            let matched = constant_time_eq(accepted, token);
            local |= index == 0 && matched;
            peer |= index != 0 && matched;
        }
        if local {
            Some(AuthenticatedMachine("local"))
        } else if peer {
            Some(AuthenticatedMachine("unknown-peer"))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthRing, AuthenticatedMachine};

    #[test]
    fn bearer_identity_is_machine_graded_without_exposing_key_material() {
        let ring = AuthRing::new("local-secret".to_string(), ["peer-secret".to_string()]);
        assert_eq!(
            ring.authenticate("Bearer local-secret"),
            Some(AuthenticatedMachine("local"))
        );
        assert_eq!(
            ring.authenticate("Bearer peer-secret"),
            Some(AuthenticatedMachine("unknown-peer"))
        );
        assert_eq!(ring.authenticate("Bearer wrong"), None);
    }
}
