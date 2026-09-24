//! Profile preference reads and writes.

use std::str::FromStr as _;

use chrono::Utc;
use rusqlite::{Connection, params};

use crate::{AxiomError, Result, app::ThinkingLevel};

use super::SessionStore;
use super::codec::{storage_error, validate_model};
use super::records::ProfilePreferences;

impl SessionStore {
    pub fn profile_preferences(&self) -> Result<ProfilePreferences> {
        let connection = self.lock()?;
        query_profile_preferences(&connection)
    }

    pub fn set_profile_preferences(
        &self,
        model: &str,
        thinking_level: ThinkingLevel,
    ) -> Result<ProfilePreferences> {
        let model = validate_model(model)?;
        let connection = self.lock()?;
        connection
            .execute(
                "UPDATE profile_preferences
                 SET selected_model=?1, thinking_level=?2, updated_at=?3 WHERE id=1",
                params![model, thinking_level.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        query_profile_preferences(&connection)
    }

    pub fn last_used_model(&self) -> Result<Option<String>> {
        Ok(self.profile_preferences()?.model)
    }

    pub fn set_last_used_model(&self, model: &str) -> Result<()> {
        let model = validate_model(model)?;
        self.lock()?
            .execute(
                "UPDATE profile_preferences SET selected_model=?1, updated_at=?2 WHERE id=1",
                params![model, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn last_used_thinking(&self) -> Result<Option<ThinkingLevel>> {
        Ok(Some(self.profile_preferences()?.thinking_level))
    }

    pub fn set_last_used_thinking(&self, level: ThinkingLevel) -> Result<()> {
        self.lock()?
            .execute(
                "UPDATE profile_preferences SET thinking_level=?1, updated_at=?2 WHERE id=1",
                params![level.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}

pub(super) fn query_profile_preferences(connection: &Connection) -> Result<ProfilePreferences> {
    let (model, thinking, updated_at): (Option<String>, String, String) = connection
        .query_row(
            "SELECT selected_model, thinking_level, updated_at
             FROM profile_preferences WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(storage_error)?;
    Ok(ProfilePreferences {
        model,
        thinking_level: ThinkingLevel::from_str(&thinking).map_err(|_| {
            AxiomError::Storage(format!(
                "invalid profile thinking level `{thinking}` in local state"
            ))
        })?,
        updated_at,
    })
}
