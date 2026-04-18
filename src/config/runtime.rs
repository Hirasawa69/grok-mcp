//! Mutable, process-lifetime state: the currently active [`ChatDefaults`].
//!
//! The defaults start as whatever [`crate::config::Config::load`] resolved, and
//! can be updated at runtime via the `grok_set_defaults` MCP tool. Mutations
//! are not persisted to disk.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::defaults::ChatDefaults;

/// Shared mutable store for [`ChatDefaults`].
///
/// Clone-and-share: the inner state is `Arc<RwLock<_>>`, so cloning only bumps
/// the reference count.
#[derive(Clone, Debug)]
pub struct RuntimeState {
    defaults: Arc<RwLock<ChatDefaults>>,
}

impl RuntimeState {
    /// Seed runtime state with the given defaults.
    #[must_use]
    pub fn new(defaults: ChatDefaults) -> Self {
        Self {
            defaults: Arc::new(RwLock::new(defaults)),
        }
    }

    /// Return a snapshot clone of the current defaults.
    pub async fn defaults(&self) -> ChatDefaults {
        self.defaults.read().await.clone()
    }

    /// Replace the entire defaults block and return the new snapshot.
    pub async fn replace(&self, new_defaults: ChatDefaults) -> ChatDefaults {
        let mut guard = self.defaults.write().await;
        *guard = new_defaults;
        guard.clone()
    }
}
