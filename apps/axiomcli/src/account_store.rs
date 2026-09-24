//! Account context owner for durable local conversation state.
//!
//! `SessionStoreRouter` starts inactive. Every exported `SessionStore` handle
//! shares its close/open barrier, so account changes invalidate already-cloned
//! handles instead of leaving a static connection to the previous account.

use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    Result,
    paths::{AxiomPaths, FrontendKind},
    session::SessionStore,
};

#[derive(Clone, Debug)]
pub struct SessionStoreRouter {
    store: SessionStore,
    generation: Arc<AtomicU64>,
}

pub struct ActiveSessionStore {
    store: SessionStore,
}

impl Deref for ActiveSessionStore {
    type Target = SessionStore;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl SessionStoreRouter {
    #[must_use]
    pub fn new(paths: AxiomPaths, frontend: FrontendKind) -> Self {
        Self {
            store: SessionStore::account_routed(paths, frontend),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Close the previous connection and open exactly one opaque account.
    /// Failures leave all cloned handles inactive.
    pub fn activate(&self, account_id: &str) -> Result<u64> {
        self.store.activate_account(account_id)?;
        Ok(self.generation.fetch_add(1, Ordering::AcqRel) + 1)
    }

    pub fn deactivate(&self) -> Result<u64> {
        self.store.deactivate_account()?;
        Ok(self.generation.fetch_add(1, Ordering::AcqRel) + 1)
    }

    pub fn active(&self) -> Result<ActiveSessionStore> {
        // Exercise the store's own authentication-required guard without
        // opening or mutating anything.
        self.store.profile_preferences()?;
        Ok(ActiveSessionStore {
            store: self.store.clone(),
        })
    }

    #[must_use]
    pub fn store(&self) -> SessionStore {
        self.store.clone()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.store.is_active()
    }

    #[must_use]
    pub fn active_account_id(&self) -> Option<String> {
        self.store.active_account_id()
    }

    pub fn active_desktop_cwd(&self) -> Result<std::path::PathBuf> {
        self.store.active_desktop_cwd()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::app::{AppCommand, Origin, PermissionProfile, Runtime, SessionId};

    use super::*;

    fn router(frontend: FrontendKind) -> (tempfile::TempDir, SessionStoreRouter) {
        let root = tempfile::tempdir().expect("temporary data root");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare paths");
        (root, SessionStoreRouter::new(paths, frontend))
    }

    #[test]
    fn signed_out_router_never_opens_a_database() {
        let (root, router) = router(FrontendKind::DesktopChat);
        assert!(!router.is_active());
        assert!(router.active().is_err());
        assert!(!root.path().join("data/accounts/account-a").exists());
        assert!(!root.path().join("data/desktop/state.sqlite3").exists());
    }

    #[tokio::test]
    async fn switching_accounts_invalidates_clones_and_never_discloses_old_catalogs() {
        let (_root, router) = router(FrontendKind::DesktopChat);
        let cloned_before_login = router.store();
        assert!(cloned_before_login.list(true).is_err());

        router.activate("account-a").expect("activate A");
        let runtime = Runtime::new(16);
        let session_id = SessionId::new();
        let created = runtime
            .dispatch(AppCommand::CreateSession {
                session_id: session_id.clone(),
                cwd: PathBuf::from("/account-a-workspace"),
                origin: Origin::Acp,
                profile: PermissionProfile::Web,
            })
            .await
            .expect("create session");
        cloned_before_login
            .append_all(&created)
            .expect("persist A session through pre-login clone");
        assert_eq!(
            router
                .active()
                .expect("A store")
                .list(true)
                .expect("A list")
                .len(),
            1
        );

        router.activate("account-b").expect("activate B");
        assert_eq!(router.active_account_id().as_deref(), Some("account-b"));
        assert!(
            cloned_before_login
                .list(true)
                .expect("B list through clone")
                .is_empty()
        );

        router.deactivate().expect("deactivate");
        assert!(cloned_before_login.list(true).is_err());
        router.activate("account-a").expect("reactivate A");
        assert_eq!(cloned_before_login.list(true).expect("A list").len(), 1);
    }

    #[test]
    fn a_failed_switch_leaves_every_clone_inactive() {
        let (root, router) = router(FrontendKind::Cli);
        let clone = router.store();
        router.activate("account-a").expect("activate A");
        let account_b = root.path().join("data/accounts/account-b");
        std::fs::create_dir_all(account_b.parent().expect("accounts directory"))
            .expect("accounts parent");
        std::fs::write(&account_b, "not a directory").expect("blocking file");
        assert!(router.activate("account-b").is_err());
        assert!(!router.is_active());
        assert!(clone.list(true).is_err());
    }
}
