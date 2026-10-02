use std::sync::Arc;

use pij_core::error::PijError;
use pij_store::spine::{RegistryCommit, RegistryPublication, RegistryPublisher};

/// Store-only tests exercise the actual transaction without a daemon channel.
/// Never invent a sequence: the callback must have committed its real SQL event.
struct CommittedPublisher;

impl RegistryPublisher for CommittedPublisher {
    fn publish_registry<'a>(&'a self, commit: RegistryCommit) -> RegistryPublication<'a> {
        Box::pin(async move {
            let (event, binding) = commit.await?;
            let seq = event.seq.ok_or_else(|| PijError::Adapter {
                adapter: "test/committed-publisher".to_string(),
                message: "registry transaction returned without a committed sequence".to_string(),
            })?;
            Ok((seq, binding))
        })
    }
}

pub fn publisher() -> Arc<dyn RegistryPublisher> {
    Arc::new(CommittedPublisher)
}
