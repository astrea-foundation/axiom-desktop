//! Connection ownership and generation-bound account routing.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;

use crate::{
    AxiomError, Result,
    app::SessionId,
    paths::{AxiomPaths, FrontendKind},
};

use super::codec::storage_error;
use super::{AccountRoute, ConnectionGuard, SessionStore, StoreBinding, StoreState};

impl SessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        let store = Self {
            state: Arc::new(Mutex::new(StoreState {
                connection: Some(connection),
                path: Some(path.to_path_buf()),
                account_id: None,
                generation: 0,
            })),
            switch: Arc::new(Mutex::new(())),
            route: None,
            binding: None,
        };
        store.initialize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        store.interrupt_incomplete_turns()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let store = Self {
            state: Arc::new(Mutex::new(StoreState {
                connection: Some(Connection::open_in_memory().map_err(storage_error)?),
                path: None,
                account_id: None,
                generation: 0,
            })),
            switch: Arc::new(Mutex::new(())),
            route: None,
            binding: None,
        };
        store.initialize()?;
        Ok(store)
    }

    /// Construct a shared store handle that starts signed out. Every clone
    /// observes the same account switch and therefore cannot retain a static
    /// installation-scoped database.
    #[must_use]
    pub fn account_routed(paths: AxiomPaths, frontend: FrontendKind) -> Self {
        Self {
            state: Arc::new(Mutex::new(StoreState {
                connection: None,
                path: None,
                account_id: None,
                generation: 0,
            })),
            switch: Arc::new(Mutex::new(())),
            route: Some(AccountRoute { paths, frontend }),
            binding: None,
        }
    }

    /// Capture the exact active store generation and account. Database methods
    /// on the returned handle validate that snapshot while holding the store
    /// mutex, so delayed work can never resolve against a later account.
    pub fn bind_active_account(&self) -> Result<Self> {
        if self.binding.is_some() {
            self.ensure_active_account_binding()?;
            return Ok(self.clone());
        }
        let state = self.lock_state()?;
        if state.connection.is_none() || (self.route.is_some() && state.account_id.is_none()) {
            return Err(authentication_required());
        }
        let binding = StoreBinding {
            generation: state.generation,
            account_id: state.account_id.clone(),
        };
        drop(state);
        Ok(Self {
            state: self.state.clone(),
            switch: self.switch.clone(),
            route: self.route.clone(),
            binding: Some(binding),
        })
    }

    /// Recheck a previously captured account binding without performing a
    /// database operation.
    pub fn ensure_active_account_binding(&self) -> Result<()> {
        let _connection = self.lock()?;
        Ok(())
    }

    /// Close the old database before opening the exact new account. Failure
    /// leaves every clone inactive and never falls back to the old store.
    pub fn activate_account(&self, account_id: &str) -> Result<()> {
        if self.binding.is_some() {
            return Err(bound_store_switch());
        }
        let route = self.route.as_ref().ok_or_else(|| {
            AxiomError::InvalidTransition("this local store is not account-routed".into())
        })?;
        let _switch = self
            .switch
            .lock()
            .map_err(|_| AxiomError::Storage("account store switch lock was poisoned".into()))?;
        {
            let mut state = self.lock_state()?;
            if state.account_id.as_deref() == Some(account_id) && state.connection.is_some() {
                return Ok(());
            }
            advance_store_generation(&mut state)?;
            state.connection = None;
            state.path = None;
            state.account_id = None;
        }
        route.paths.prepare_account(account_id)?;
        let path = route
            .paths
            .account_state_database(account_id, route.frontend)?;
        let candidate = Self::open(&path)?;
        let connection = candidate
            .lock_state()?
            .connection
            .take()
            .ok_or_else(|| AxiomError::Storage("new account database did not open".into()))?;
        let mut state = self.lock_state()?;
        state.connection = Some(connection);
        state.path = Some(path);
        state.account_id = Some(account_id.to_owned());
        Ok(())
    }

    pub fn deactivate_account(&self) -> Result<()> {
        if self.binding.is_some() {
            return Err(bound_store_switch());
        }
        let _switch = self
            .switch
            .lock()
            .map_err(|_| AxiomError::Storage("account store switch lock was poisoned".into()))?;
        let mut state = self.lock_state()?;
        advance_store_generation(&mut state)?;
        state.connection = None;
        state.path = None;
        state.account_id = None;
        Ok(())
    }

    #[must_use]
    pub fn active_account_id(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| {
                binding_matches(self.binding.as_ref(), &state).then(|| state.account_id.clone())
            })
            .flatten()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.connection.is_some()
                && (self.route.is_none() || state.account_id.is_some())
                && binding_matches(self.binding.as_ref(), &state)
        })
    }

    pub fn active_desktop_cwd(&self) -> Result<PathBuf> {
        let route = self.route.as_ref().ok_or_else(|| {
            AxiomError::InvalidTransition("this local store is not account-routed".into())
        })?;
        if route.frontend != FrontendKind::DesktopChat {
            return Err(AxiomError::InvalidTransition(
                "desktop workspace requested for a non-desktop account store".into(),
            ));
        }
        let account_id = self
            .active_account_id()
            .ok_or_else(authentication_required)?;
        route.paths.account_desktop_chat_cwd(&account_id)
    }

    pub fn active_desktop_thread_cwd(&self, thread: &SessionId) -> Result<PathBuf> {
        self.active_desktop_cwd()?;
        let route = self.route.as_ref().ok_or_else(authentication_required)?;
        route.paths.account_desktop_thread_cwd(
            &self
                .active_account_id()
                .ok_or_else(authentication_required)?,
            thread,
        )
    }
}

pub(super) fn binding_matches(binding: Option<&StoreBinding>, state: &StoreState) -> bool {
    binding.is_none_or(|binding| {
        binding.generation == state.generation && binding.account_id == state.account_id
    })
}

pub(super) fn advance_store_generation(state: &mut StoreState) -> Result<()> {
    state.generation = state
        .generation
        .checked_add(1)
        .ok_or_else(|| AxiomError::Storage("account store generation overflowed".into()))?;
    Ok(())
}

pub(super) fn stale_account_binding() -> AxiomError {
    AxiomError::InvalidTransition(
        "account-scoped local store changed while the operation was pending".into(),
    )
}

pub(super) fn bound_store_switch() -> AxiomError {
    AxiomError::InvalidTransition(
        "an account-bound local store handle cannot change the active account".into(),
    )
}

pub(super) fn authentication_required() -> AxiomError {
    AxiomError::InvalidTransition(
        "authentication required; no account-scoped local store is active".into(),
    )
}

impl SessionStore {
    pub(super) fn lock_state(&self) -> Result<MutexGuard<'_, StoreState>> {
        self.state
            .lock()
            .map_err(|_| AxiomError::Storage("local state database lock was poisoned".into()))
    }

    pub(super) fn lock(&self) -> Result<ConnectionGuard<'_>> {
        let guard = self.lock_state()?;
        if !binding_matches(self.binding.as_ref(), &guard) {
            return Err(stale_account_binding());
        }
        if guard.connection.is_none() {
            return Err(authentication_required());
        }
        Ok(ConnectionGuard { guard })
    }
}
