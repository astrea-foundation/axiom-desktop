//! Durable local conversation state.
//!
//! `SQLite` stores stable thread, turn, timeline, preference, and collection
//! records. [`AppEvent`](crate::app::AppEvent) remains the runtime vocabulary, but event envelopes
//! are translated at this boundary and are never the on-disk contract.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;

use crate::paths::{AxiomPaths, FrontendKind};

mod accounting;
mod codec;
mod collections;
mod connection;
mod events;
mod preferences;
mod projection;
mod queries;
mod records;
mod recovery;
mod schema;
mod settings;
#[cfg(test)]
mod tests;
mod timeline;

pub use codec::{model_for_resume, permission_for_resume, thinking_for_resume};
pub use records::{
    Collection, CollectionChange, CollectionState, ExportOptions, InterruptedTool, LoadOutcome,
    ProfilePreferences, RecoveryStatus, SessionSummary, ThreadCatalogPage, ThreadLifecycle,
    ThreadRevision, ThreadSnapshot, ThreadSummary, TimelineItem, TimelineItemKind,
    TimelineItemStatus, TurnRecord, TurnStatus,
};

const MAX_TIMELINE_PAGE: usize = 1_000;
const MAX_THREAD_CATALOG_PAGE: usize = 200;
const MAX_THREAD_QUERY_BYTES: usize = 512;
const MAX_TIMELINE_ITEMS_PER_LOAD: usize = 100_000;
const DEFAULT_EXPORT_ITEMS: usize = 10_000;
const MAX_CLIENT_ITEM_ID_BYTES: usize = 512;
const MAX_COLLECTION_NAME_BYTES: usize = 120;

#[derive(Clone)]
pub struct SessionStore {
    state: Arc<Mutex<StoreState>>,
    switch: Arc<Mutex<()>>,
    route: Option<AccountRoute>,
    binding: Option<StoreBinding>,
}

struct StoreState {
    connection: Option<Connection>,
    path: Option<PathBuf>,
    account_id: Option<String>,
    generation: u64,
}

#[derive(Clone, Debug)]
struct AccountRoute {
    paths: AxiomPaths,
    frontend: FrontendKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreBinding {
    generation: u64,
    account_id: Option<String>,
}

struct ConnectionGuard<'a> {
    guard: MutexGuard<'a, StoreState>,
}

impl std::ops::Deref for ConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.guard
            .connection
            .as_ref()
            .expect("connection guard is created only for active stores")
    }
}

impl std::ops::DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard
            .connection
            .as_mut()
            .expect("connection guard is created only for active stores")
    }
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionStore")
            .field(
                "path",
                &self.state.lock().ok().and_then(|state| state.path.clone()),
            )
            .field("account_routed", &self.route.is_some())
            .field("account_bound", &self.binding.is_some())
            .finish_non_exhaustive()
    }
}
