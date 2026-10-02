use std::sync::Arc;

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{SeatDescriptor, SeatId, Seq};
use pij_core::ports::{PutBinding, Registry, SeatFilter};
use pij_store::registry::{activity_event, descriptor_event, tombstone_event};
use pij_store::spine::RegistryPublisher;
use pij_testkit::fakes::FakeRegistry;

use super::EventBus;

/// Publish in-memory registry mutations through the daemon's one event bus.
///
/// The concrete fake's writes cannot fail. Append first, then mutate, then
/// broadcast under the shared publication lock. Its private counter is never
/// exposed: every Registry result refers to the bus's durable sequence domain.
/// Do not use this wrapper for SQLite, which has its own atomic transaction.
///
/// Composition: `PublishedFakeRegistry::new(raw_fake, event_bus.clone())`.
pub struct PublishedFakeRegistry {
    registry: Arc<FakeRegistry>,
    bus: Arc<EventBus>,
}

impl PublishedFakeRegistry {
    /// Wrap an in-memory fake with the same bus exposed as `Services.spine`.
    pub fn new(registry: Arc<FakeRegistry>, bus: Arc<EventBus>) -> Self {
        Self { registry, bus }
    }
    async fn publish_tombstone(
        &self,
        seat: SeatId,
        reason: String,
        expected: Option<SeatDescriptor>,
    ) -> Result<Seq> {
        let registry = self.registry.clone();
        let spine = self.bus.spine.clone();
        self.bus.publish_registry(Box::pin(async move {
            // All composed fake writers share this bus ordering boundary. The
            // raw fake is not an alternate production write entry point.
            let current = registry.get(&seat).await?;
            if let Some(expected) = expected.as_ref()
                && current.as_ref() != Some(expected)
            {
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-INCARNATION-CHANGED".to_string(),
                    record: seat.to_string(),
                });
            }
            let mut descriptor = current.ok_or_else(|| PijError::NoRegistryEntry {
                seat: seat.clone(),
                store: "the daemon's in-memory fake registry".to_string(),
            })?;
            let mut event = tombstone_event(&seat, &reason, expected.as_ref())?;
            descriptor.tombstoned_at = Some(event.at);
            descriptor.tombstone_reason = Some(reason);
            descriptor.native_extension_delivery = false;
            if descriptor.state == pij_core::model::SystemState::Working {
                descriptor.state = pij_core::model::SystemState::Idle;
            }
            let seq = spine.append(event.clone()).await?;
            registry.put(descriptor).await.map_err(|error| PijError::Adapter {
                adapter: "daemon/events".to_string(),
                message: format!("fake registry event {} committed but tombstone mutation failed: {error}", seq.0),
            })?;
            event.seq = Some(seq);
            Ok((event, None))
        })).await.map(|(seq, _)| seq)
    }
}

#[async_trait]
impl Registry for PublishedFakeRegistry {
    async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
        self.registry.get(seat).await
    }

    async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
        self.put_reporting(descriptor).await.map(|(seq, _)| seq)
    }

    async fn put_reporting(&self, descriptor: SeatDescriptor) -> Result<(Seq, PutBinding)> {
        let registry = self.registry.clone();
        let spine = self.bus.spine.clone();
        let (seq, binding) = self.bus.publish_registry(Box::pin(async move {
            let mut descriptor = descriptor;
            descriptor.machine = None;
            let mut event = descriptor_event(&descriptor)?;
            let seq = spine.append(event.clone()).await?;
            let (_, binding) = registry.put_reporting(descriptor).await.map_err(|error| PijError::Adapter {
                adapter: "daemon/events".to_string(),
                message: format!("fake registry event {} committed but descriptor mutation failed: {error}", seq.0),
            })?;
            event.seq = Some(seq);
            Ok((event, Some(binding)))
        })).await?;
        let binding = binding.ok_or_else(|| PijError::Adapter {
            adapter: "daemon/events".to_string(),
            message: "fake registry publication lost the committed put binding".to_string(),
        })?;
        Ok((seq, binding))
    }

    async fn list(&self, filter: SeatFilter) -> Result<Vec<SeatDescriptor>> {
        self.registry.list(filter).await
    }

    async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
        self.publish_tombstone(seat.clone(), reason.to_string(), None)
            .await
    }

    async fn tombstone_if_unchanged(
        &self,
        expected: SeatDescriptor,
        reason: String,
    ) -> Result<Seq> {
        self.publish_tombstone(expected.id.clone(), reason, Some(expected))
            .await
    }

    async fn set_activity(
        &self,
        seat: &SeatId,
        state: pij_core::model::SystemState,
        reason: Option<&str>,
    ) -> Result<Option<Seq>> {
        match self.registry.get(seat).await? {
            Some(current) if current.tombstoned_at.is_none() && current.state != state => {}
            _ => return Ok(None),
        }
        let registry = self.registry.clone();
        let spine = self.bus.spine.clone();
        let seat = seat.clone();
        let reason = reason.map(str::to_string);
        let published = self
            .bus
            .publish_registry(Box::pin(async move {
                let mut event = activity_event(&seat, state, reason.as_deref())?;
                let seq = spine.append(event.clone()).await?;
                if registry
                    .set_activity(&seat, state, reason.as_deref())
                    .await?
                    .is_none()
                {
                    return Err(PijError::SeatIsGone {
                        seat,
                        tombstone_reason: None,
                    });
                }
                event.seq = Some(seq);
                Ok((event, None))
            }))
            .await;
        match published {
            Ok((seq, _)) => Ok(Some(seq)),
            Err(PijError::SeatIsGone { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
