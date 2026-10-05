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

/// Answers [`ColdRouting`] from the registry, transcripts, roles, governance and queue.
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

impl DaemonColdRouting {
    /// The prime of the project behind a seat's newest open task assignment.
    async fn project_prime(&self, seat: &SeatId) -> Result<Option<SeatId>> {
        let task = self
            .orchestration
            .list_tasks(Some(seat))
            .await?
            .into_iter()
            .filter(|task| task.closed_at.is_none() && task.project.is_some())
            .max_by_key(|task| task.opened_at);
        let Some(slug) = task.and_then(|task| task.project) else {
            return Ok(None);
        };
        Ok(self
            .orchestration
            .project(&slug)
            .await?
            .and_then(|project| project.prime_id))
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
        let mut project_prime = self.project_prime(&seat.id).await?;
        let mut seen = HashSet::from([seat.id.clone()]);
        let mut next = seat.parent.clone();
        while let Some(id) = next {
            if !seen.insert(id.clone()) {
                break;
            }
            let Some(ancestor) = self.registry.get(&id).await? else {
                break;
            };
            // The role store is the only authority (join_roles overwrites every
            // descriptor role from it): a descriptor role is never consulted, so
            // an explicit unset cannot be resurrected from a stale copy.
            let role = self.roles.read_role(&id).await?;
            if project_prime.is_none() {
                project_prime = self.project_prime(&id).await?;
            }
            ancestors.push((id, role));
            next = ancestor.parent;
        }
        let designated = self
            .orchestration
            .prime()
            .await?
            .filter(|designation| designation.state == PrimeState::Current)
            .map(|designation| designation.seat);
        Ok(nearest_prime(
            &seat.id,
            &ancestors,
            project_prime,
            designated,
        ))
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

/// The nearest ancestor (nearest first) whose role is prime, else the prime of
/// the seat's project (its open task, or its nearest ancestor's), else the
/// machine-wide designated prime; never the seat itself.
fn nearest_prime(
    seat: &SeatId,
    ancestors: &[(SeatId, Option<String>)],
    project_prime: Option<SeatId>,
    designated: Option<SeatId>,
) -> Option<SeatId> {
    ancestors
        .iter()
        .find(|(_, role)| role.as_deref() == Some(PRIME_ROLE))
        .map(|(id, _)| id.clone())
        .or(project_prime)
        .or(designated)
        .filter(|prime| prime != seat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use pij_core::model::Harness;
    use pij_core::orchestration::{PrimeDesignation, Project, TaskAssignment};
    use pij_testkit::fakes::{FakeQueue, FakeRegistry, FakeSessionStatus, FakeSpine};

    /// A seat with a parent, a project prime reachable through its open task,
    /// and a different machine-wide designated prime.
    async fn routing(task_holder: &str, close_task: bool) -> (DaemonColdRouting, SeatDescriptor) {
        let pool = pij_store::open("").await.unwrap();
        let orchestration = SqliteOrchestration::new(pool.clone());
        let registry = Arc::new(FakeRegistry::new());
        let mut owner = SeatDescriptor::new("coder", Harness::Claude, "/tmp");
        owner.parent = Some(id("pm"));
        registry.put(owner.clone()).await.unwrap();
        registry
            .put(SeatDescriptor::new("pm", Harness::Claude, "/tmp"))
            .await
            .unwrap();
        assert!(
            orchestration
                .create_project(&Project {
                    slug: "proj".to_string(),
                    description: None,
                    repo: None,
                    plan_path: None,
                    prime_id: Some(id("project-prime")),
                    created_by: id("pm"),
                    created_at: 1,
                })
                .await
                .unwrap()
        );
        assert!(
            orchestration
                .open_task(&TaskAssignment {
                    id: "task-1".to_string(),
                    node_id: id(task_holder),
                    task: "build".to_string(),
                    project: Some("proj".to_string()),
                    opened_by: id("pm"),
                    opened_at: 2,
                    closed_at: None,
                    close_reason: None,
                })
                .await
                .unwrap()
        );
        if close_task {
            orchestration
                .close_task("task-1", pij_core::orchestration::TaskCloseReason::Done, 3)
                .await
                .unwrap();
        }
        orchestration
            .designate_prime(&PrimeDesignation {
                seat: id("machine-prime"),
                designated_by: id("pm"),
                designated_at: 4,
                state: PrimeState::Current,
            })
            .await
            .unwrap();
        let spine = Arc::new(FakeSpine::new());
        let roles = Arc::new(RoleService::new(
            registry.clone(),
            SqliteOrchestration::new(pool),
            Arc::new(EventBus::new(spine, 16).unwrap()),
        ));
        let routing = DaemonColdRouting::new(
            registry,
            Arc::new(FakeSessionStatus::new()),
            roles,
            orchestration,
            Arc::new(FakeQueue::new(8).unwrap()),
        );
        (routing, owner)
    }

    #[tokio::test]
    async fn the_owners_project_prime_comes_before_the_machine_designation() {
        // Review B3 (#22): an open TaskAssignment links the seat to a project,
        // and that project's prime_id is "the project's designated prime".
        let (routing, owner) = routing("coder", false).await;
        assert_eq!(
            routing.prime(&owner).await.unwrap(),
            Some(id("project-prime"))
        );
    }

    #[tokio::test]
    async fn an_ancestors_open_task_supplies_the_project_when_the_owner_has_none() {
        let (routing, owner) = routing("pm", false).await;
        assert_eq!(
            routing.prime(&owner).await.unwrap(),
            Some(id("project-prime"))
        );
    }

    #[tokio::test]
    async fn an_explicitly_unset_role_is_not_resurrected_from_a_stale_descriptor() {
        // Review B6 (#22): the role store is the authority (join_roles replaces
        // every descriptor role from it); a cleared role must exclude the seat.
        let pool = pij_store::open("").await.unwrap();
        let orchestration = SqliteOrchestration::new(pool.clone());
        let registry = Arc::new(FakeRegistry::new());
        let mut owner = SeatDescriptor::new("coder", Harness::Claude, "/tmp");
        owner.parent = Some(id("pm"));
        registry.put(owner.clone()).await.unwrap();
        let mut stale = SeatDescriptor::new("pm", Harness::Claude, "/tmp");
        stale.role = Some("prime".to_string());
        registry.put(stale).await.unwrap();
        orchestration
            .assign_role(&pij_core::orchestration::RoleAssignment {
                seat: id("pm"),
                role: "prime".to_string(),
                assigned_by: id("pm"),
                assigned_at: 1,
            })
            .await
            .unwrap();
        orchestration.clear_role(&id("pm")).await.unwrap();
        let roles = Arc::new(RoleService::new(
            registry.clone(),
            SqliteOrchestration::new(pool),
            Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 16).unwrap()),
        ));
        let routing = DaemonColdRouting::new(
            registry,
            Arc::new(FakeSessionStatus::new()),
            roles,
            orchestration,
            Arc::new(FakeQueue::new(8).unwrap()),
        );
        assert_eq!(routing.prime(&owner).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_closed_task_no_longer_links_the_project() {
        let (routing, owner) = routing("coder", true).await;
        assert_eq!(
            routing.prime(&owner).await.unwrap(),
            Some(id("machine-prime"))
        );
    }

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
            nearest_prime(
                &id("coder"),
                &ancestors,
                Some(id("project")),
                Some(id("designated"))
            ),
            Some(id("near-prime"))
        );
    }

    #[test]
    fn without_a_prime_ancestor_the_designation_is_used_but_never_the_seat_itself() {
        let ancestors = [(id("pm"), None)];
        assert_eq!(
            nearest_prime(&id("coder"), &ancestors, None, Some(id("designated"))),
            Some(id("designated"))
        );
        assert_eq!(
            nearest_prime(
                &id("coder"),
                &ancestors,
                Some(id("project")),
                Some(id("designated"))
            ),
            Some(id("project"))
        );
        assert_eq!(nearest_prime(&id("coder"), &ancestors, None, None), None);
        assert_eq!(
            nearest_prime(&id("designated"), &[], None, Some(id("designated"))),
            None
        );
    }
}
