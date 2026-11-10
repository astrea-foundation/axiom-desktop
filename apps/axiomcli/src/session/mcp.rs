use super::{SessionStore, codec::storage_error};
use crate::{
    AxiomError, Result,
    app::SessionId,
    desktop_mcp::{DesktopMcpState, response},
};
use axiom_acp_extension::{DesktopMcpResponse, DesktopMcpServer};
use rusqlite::OptionalExtension;

impl SessionStore {
    pub fn desktop_mcp_snapshot(&self, thread: Option<&SessionId>) -> Result<DesktopMcpResponse> {
        let connection = self.lock()?;
        let text: String = connection
            .query_row(
                "SELECT payload FROM desktop_mcp_state WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let state: DesktopMcpState = serde_json::from_str(&text)?;
        let selected = if let Some(thread) = thread {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)",
                    [thread.to_string()],
                    |r| r.get(0),
                )
                .map_err(storage_error)?;
            if !exists {
                return Err(AxiomError::Storage("unknown MCP thread".into()));
            }
            let selected: Option<String> = connection
                .query_row(
                    "SELECT tools FROM desktop_mcp_threads WHERE thread_id=?1",
                    [thread.to_string()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            selected
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(response(state, selected))
    }

    pub(crate) fn save_desktop_mcp(
        &self,
        expected: u64,
        servers: Vec<DesktopMcpServer>,
        selection: Option<(&SessionId, Vec<String>)>,
    ) -> Result<()> {
        if servers
            .iter()
            .map(|server| server.tools.len())
            .sum::<usize>()
            > 512
            || serde_json::to_vec(&servers)?.len() > 4 * 1024 * 1024
        {
            return Err(AxiomError::Config("MCP connections exceed the account limit of 512 discovered tools or 4 MiB of configuration".into()));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let text: String = transaction
            .query_row(
                "SELECT payload FROM desktop_mcp_state WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        let mut state: DesktopMcpState = serde_json::from_str(&text)?;
        if state.revision != expected {
            return Err(AxiomError::InvalidTransition(
                "MCP settings changed; refresh before saving".into(),
            ));
        }
        state.revision = expected
            .checked_add(1)
            .ok_or_else(|| AxiomError::Storage("MCP revision overflow".into()))?;
        let previous = state.servers.clone();
        state.servers = servers.into_iter().map(|s| (s.name.clone(), s)).collect();
        if let Some((thread, tools)) = selection {
            let available: std::collections::BTreeSet<_> = state
                .servers
                .values()
                .filter(|s| s.enabled)
                .flat_map(|s| s.tools.iter().map(|t| &t.name))
                .collect();
            if tools.len() > 256 || tools.iter().any(|t| !available.contains(t)) {
                return Err(AxiomError::Config(
                    "select only tools from enabled, tested MCP connections".into(),
                ));
            }
            transaction.execute("INSERT INTO desktop_mcp_threads(thread_id,tools) VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET tools=excluded.tools", rusqlite::params![thread.to_string(), serde_json::to_string(&tools)?]).map_err(storage_error)?;
        } else {
            // Only changed server authority invalidates its tools; other threads'
            // connections and selections survive unrelated edits.
            let changed: Vec<_> = previous
                .values()
                .filter(|old| {
                    state.servers.get(&old.name).is_none_or(|new| {
                        new.command != old.command
                            || new.args != old.args
                            || new.enabled != old.enabled
                            || new.credential_id != old.credential_id
                            || new.tools != old.tools
                    })
                })
                .map(|s| format!("mcp__{}__", s.name))
                .collect();
            let rows = {
                let mut statement = transaction
                    .prepare("SELECT thread_id, tools FROM desktop_mcp_threads")
                    .map_err(storage_error)?;
                statement
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map_err(storage_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(storage_error)?
            };
            for (id, text) in rows {
                let mut selected: Vec<String> = serde_json::from_str(&text)?;
                selected.retain(|tool| !changed.iter().any(|prefix| tool.starts_with(prefix)));
                transaction
                    .execute(
                        "UPDATE desktop_mcp_threads SET tools=?2 WHERE thread_id=?1",
                        rusqlite::params![id, serde_json::to_string(&selected)?],
                    )
                    .map_err(storage_error)?;
            }
        }
        transaction
            .execute(
                "UPDATE desktop_mcp_state SET payload=?1 WHERE id=1",
                [serde_json::to_string(&state)?],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(())
    }
}
