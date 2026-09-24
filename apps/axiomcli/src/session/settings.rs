//! Account-scoped Desktop Agent settings.

use std::path::{Path, PathBuf};

use crate::{AxiomError, Result, app::SessionId};

use super::SessionStore;
use super::codec::storage_error;

impl SessionStore {
    pub fn desktop_agent_settings(
        &self,
        thread: &SessionId,
    ) -> Result<axiom_acp_extension::DesktopAgentSettings> {
        let default = self.active_desktop_thread_cwd(thread)?;
        let connection = self.lock()?;
        let stored: Option<String> = connection
            .query_row(
                "SELECT desktop_agent_settings FROM threads WHERE id=?1",
                [thread.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if let Some(stored) = stored {
            return serde_json::from_str(&stored).map_err(|error| {
                AxiomError::Storage(format!("invalid saved Agent settings: {error}"))
            });
        }
        let default = default
            .to_str()
            .ok_or_else(|| AxiomError::Storage("desktop workspace must be valid Unicode".into()))?
            .to_owned();
        Ok(axiom_acp_extension::DesktopAgentSettings {
            enabled: false,
            permission: axiom_acp_extension::DesktopAgentPermission::ApproveCommands,
            working_directory: default.clone(),
            default_working_directory: default,
            uses_default_directory: true,
            revision: 0,
        })
    }

    pub fn prepare_desktop_agent_settings(
        &self,
        thread: &SessionId,
        request: &axiom_acp_extension::ConfigureDesktopAgentRequest,
    ) -> Result<axiom_acp_extension::DesktopAgentSettings> {
        let current = self.desktop_agent_settings(thread)?;
        if current.revision != request.expected_revision {
            return Err(AxiomError::InvalidTransition(
                "Agent settings changed; refresh before applying".into(),
            ));
        }
        let default = self.active_desktop_thread_cwd(thread)?;
        let chosen = match &request.working_directory {
            None => default.clone(),
            Some(path) => {
                if path.is_empty()
                    || path.len() > 32_768
                    || path.contains('\0')
                    || !Path::new(path).is_absolute()
                {
                    return Err(AxiomError::InvalidTransition(
                        "working directory must be an absolute path".into(),
                    ));
                }
                let path = PathBuf::from(path).canonicalize()?;
                if !path.is_dir() {
                    return Err(AxiomError::InvalidTransition(
                        "working directory is not a folder".into(),
                    ));
                }
                path
            }
        };
        let to_text = |path: &Path| {
            path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                AxiomError::InvalidTransition("working directory must be valid Unicode".into())
            })
        };
        Ok(axiom_acp_extension::DesktopAgentSettings {
            enabled: request.enabled,
            permission: request.permission,
            uses_default_directory: chosen == default,
            working_directory: to_text(&chosen)?,
            default_working_directory: to_text(&default)?,
            revision: current
                .revision
                .checked_add(1)
                .ok_or_else(|| AxiomError::Storage("Agent settings revision overflow".into()))?,
        })
    }
}
