//! Production facts for routing an event source's batch away from a cold owner.
//!
//! [`BackgroundService`](crate::background::BackgroundService) owns the routing
//! decision; this adapter only answers its three questions from the daemon's
//! real ports: is a seat cold (the cold-wake guard's own `check()`), who is its
//! prime, and how to reach the human (the Telegram sidecar queue).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pij_core::cold_wake::{ColdCheck, check};
use pij_core::error::Result;
use pij_core::model::{SeatDescriptor, SeatId};
use pij_core::orchestration::PrimeState;
use pij_core::ports::{Queue, Registry, SessionStatusPort};
use pij_core::session_status::SessionStatusBlock;
use pij_sidecars::telegram::{TelegramSend, send_job};
use pij_store::SqliteOrchestration;

use crate::background::ColdRouting;
use crate::http::role::RoleService;
use crate::http::{session_status_block, system_time_ms};

/// The same bound the send guard puts on a transcript read.
const STATUS_WAIT: Duration = Duration::from_secs(3);
const PRIME_ROLE: &str = "prime";

/// Answers [`ColdRouting`] from the registry, transcripts, roles and queue.
pub struct DaemonColdRouting {
    registry: Arc<dyn Registry>,
    session_status: Arc<dyn SessionStatusPort>,
    roles: Arc<RoleService>,
    orchestration: SqliteOrchestration,
    queue: Arc<dyn Queue>,
}

impl DaemonColdRouting {
    /// Compose from the daemon's shared ports.
    pub fn new(
        registry: Arc<dyn Registry>,
        session_status: Arc<dyn SessionStatusPort>,
        roles: Arc<RoleService>,
        orchestration: SqliteOrchestration,
        queue: Arc<dyn Queue>,
    ) -> Self {
        Self {
            registry,
            session_status,
            roles,
            orchestration,
            queue,
        }
    }
}

#[async_trait]
impl ColdRouting for DaemonColdRouting {
    async fn check(&self, seat: &SeatDescriptor) -> ColdCheck {
        let now = match system_time_ms() {
            Ok(now) => now,
            Err(error) => {
                return ColdCheck::Unknown {
                    why: error.to_string(),
                };
            }
        };
        let block = tokio::time::timeout(
            STATUS_WAIT,
            session_status_block(
                self.session_status.as_ref(),
                &seat.id,
                seat.harness,
                seat.harness_session.clone(),
                now,
            ),
        )
        .await
        .unwrap_or_else(|_| SessionStatusBlock::Failed {
            error: format!("no answer within {}s", STATUS_WAIT.as_secs()),
        });
        check(seat.state, &block, now)
    }

    async fn prime(&self, seat: &SeatDescriptor) -> Result<Option<SeatId>> {
        let mut ancestors = Vec::new();
        let mut seen = HashSet::from([seat.id.clone()]);
        let mut next = seat.parent.clone();
        while let Some(id) = next {
            if !seen.insert(id.clone()) {
                break;
            }
            let Some(ancestor) = self.registry.get(&id).await? else {
                break;
            };
            let role = self.roles.read_role(&id).await?.or(ancestor.role.clone());
            ancestors.push((id, role));
            next = ancestor.parent;
        }
        let designated = self
            .orchestration
            .prime()
            .await?
            .filter(|designation| designation.state == PrimeState::Current)
            .map(|designation| designation.seat);
        Ok(nearest_prime(&seat.id, &ancestors, designated))
    }

    async fn telegram(&self, from: &SeatId, body: String, msg_id: String) -> Result<()> {
        self.queue
            .enqueue(send_job(&TelegramSend {
                from: from.clone(),
                body,
                msg_id,
                chat_id: None,
            })?)
            .await
            .map(|_| ())
    }
}

/// The nearest ancestor (nearest first) whose role is prime, else the designated
/// prime; never the seat itself.
fn nearest_prime(
    seat: &SeatId,
    ancestors: &[(SeatId, Option<String>)],
    designated: Option<SeatId>,
) -> Option<SeatId> {
    ancestors
        .iter()
        .find(|(_, role)| role.as_deref() == Some(PRIME_ROLE))
        .map(|(id, _)| id.clone())
        .or(designated)
        .filter(|prime| prime != seat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> SeatId {
        SeatId::from(name)
    }

    #[test]
    fn the_nearest_prime_ancestor_wins_over_the_designation() {
        let ancestors = [
            (id("pm"), Some("pm".to_string())),
            (id("near-prime"), Some("prime".to_string())),
            (id("far-prime"), Some("prime".to_string())),
        ];
        assert_eq!(
            nearest_prime(&id("coder"), &ancestors, Some(id("designated"))),
            Some(id("near-prime"))
        );
    }

    #[test]
    fn without_a_prime_ancestor_the_designation_is_used_but_never_the_seat_itself() {
        let ancestors = [(id("pm"), None)];
        assert_eq!(
            nearest_prime(&id("coder"), &ancestors, Some(id("designated"))),
            Some(id("designated"))
        );
        assert_eq!(nearest_prime(&id("coder"), &ancestors, None), None);
        assert_eq!(
            nearest_prime(&id("designated"), &[], Some(id("designated"))),
            None
        );
    }
}
