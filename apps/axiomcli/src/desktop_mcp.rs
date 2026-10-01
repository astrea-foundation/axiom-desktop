//! Desktop-owned local MCP configuration. No tool payload crosses the hosted backend.
use crate::{AxiomError, Result, app::SessionId, config::McpServerConfig, session::SessionStore};
use axiom_acp_extension::{DesktopMcpResponse, DesktopMcpServer, DesktopMcpServerInput};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DesktopMcpState {
    pub revision: u64,
    pub servers: BTreeMap<String, DesktopMcpServer>,
}

/// OS credential calls cannot be cancelled. Bound the wait; immutable entries
/// ensure a late write cannot change an already accepted server configuration.
pub(crate) async fn credential_task<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::task::spawn_blocking(operation),
    )
    .await
    .map_err(|_| AxiomError::Storage("MCP system credential store timed out".into()))?
    .map_err(|_| AxiomError::Storage("MCP credential operation failed".into()))?
}

#[derive(Clone, Default)]
pub struct DesktopMcpTurn {
    pub servers: Vec<McpServerConfig>,
    pub tools: BTreeSet<String>,
    pub schemas: BTreeMap<String, String>,
}

pub(crate) fn validate_input(input: &DesktopMcpServerInput) -> Result<()> {
    if input.name.is_empty()
        || input.name.len() > 48
        || !input
            .name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        || input.name.starts_with('_')
        || input.name.ends_with('_')
    {
        return Err(AxiomError::Config(
            "MCP names must use lowercase letters, numbers and underscores (1–48 characters)"
                .into(),
        ));
    }
    if input.command.trim().is_empty()
        || input.command.len() > 4096
        || input.command.contains(['\0', '\n', '\r'])
        || input.args.len() > 128
        || input
            .args
            .iter()
            .any(|s| s.len() > 8192 || s.contains('\0'))
    {
        return Err(AxiomError::Config(
            "invalid MCP command or arguments".into(),
        ));
    }
    if let Some(env) = &input.env
        && (env.len() > 64
            || env.iter().any(|(name, value)| {
                name.is_empty()
                    || name.len() > 128
                    || name.as_bytes()[0].is_ascii_digit()
                    || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || value.len() > 16_384
                    || value.contains('\0')
            }))
    {
        return Err(AxiomError::Config(
            "invalid MCP environment variables".into(),
        ));
    }
    Ok(())
}

fn credential_entry(store: &SessionStore, name: &str) -> Result<keyring::Entry> {
    let account_root = store.active_desktop_cwd()?;
    let scope = Sha256::digest(account_root.as_os_str().to_string_lossy().as_bytes());
    keyring::Entry::new("stream.axiom.desktop.mcp", &format!("{scope:x}:{name}"))
        .map_err(|_| AxiomError::Storage("MCP system credential store is unavailable".into()))
}

pub(crate) fn write_environment(
    store: &SessionStore,
    name: &str,
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let entry = credential_entry(store, name)?;
    if env.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AxiomError::Storage(
                "could not clear MCP credentials in the system credential store".into(),
            )),
        };
    }
    entry.set_password(&serde_json::to_string(env)?)
        .map_err(|_| AxiomError::Storage("could not save MCP credentials in the system credential store; no plaintext fallback is used".into()))
}

pub(crate) fn launch_config(
    store: &SessionStore,
    server: &DesktopMcpServer,
) -> Result<McpServerConfig> {
    let env = if server.environment_keys.is_empty() {
        BTreeMap::new()
    } else {
        let id = server
            .credential_id
            .as_ref()
            .ok_or_else(|| AxiomError::Storage("MCP credential reference is missing".into()))?;
        let secret = credential_entry(store, &format!("{}:{id}", server.name))?.get_password()
            .map_err(|_| AxiomError::Storage("MCP credentials are missing or locked; edit the connection and save them again".into()))?;
        let env: BTreeMap<String, String> = serde_json::from_str(&secret).map_err(|_| {
            AxiomError::Storage("invalid MCP credentials in the system credential store".into())
        })?;
        if env.keys().cloned().collect::<Vec<_>>() != server.environment_keys {
            return Err(AxiomError::Storage(
                "MCP credentials changed; save the connection again".into(),
            ));
        }
        env
    };
    Ok(McpServerConfig {
        name: server.name.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
        env,
        read_only_tools: Vec::new(),
        tool_timeout_secs: 300,
    })
}

pub fn turn_config(
    store: &SessionStore,
    thread: &SessionId,
    expected: u64,
) -> Result<Option<DesktopMcpTurn>> {
    let response = store.desktop_mcp_snapshot(Some(thread))?;
    if response.revision != expected {
        return Err(AxiomError::InvalidTransition(
            "MCP settings changed since this message was queued. Review and resend it.".into(),
        ));
    }
    if response.selected_tools.is_empty() {
        return Ok(None);
    }
    let tools: BTreeSet<_> = response.selected_tools.into_iter().collect();
    let mut servers = Vec::new();
    let mut schemas = BTreeMap::new();
    for server in response.servers {
        if server.enabled && server.tools.iter().any(|tool| tools.contains(&tool.name)) {
            for tool in &server.tools {
                if tools.contains(&tool.name) {
                    schemas.insert(tool.name.clone(), tool.schema_hash.clone());
                }
            }
            servers.push(launch_config(store, &server)?);
        }
    }
    Ok(Some(DesktopMcpTurn {
        servers,
        tools,
        schemas,
    }))
}

#[must_use]
pub fn schema_hash(value: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(crate) fn response(state: DesktopMcpState, selected_tools: Vec<String>) -> DesktopMcpResponse {
    DesktopMcpResponse {
        revision: state.revision,
        servers: state.servers.into_values().collect(),
        selected_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_input_is_bounded_and_debug_omits_credentials() {
        let mut input = DesktopMcpServerInput {
            name: "local".into(),
            command: "server".into(),
            args: vec![],
            enabled: true,
            env: Some(BTreeMap::from([(
                "TOKEN".into(),
                "secret-token-value".into(),
            )])),
        };
        validate_input(&input).unwrap();
        assert!(!format!("{input:?}").contains("secret-token-value"));
        input
            .env
            .as_mut()
            .unwrap()
            .insert("INVALID=NAME".into(), "value".into());
        assert!(validate_input(&input).is_err());
        assert!(serde_json::from_value::<DesktopMcpServerInput>(serde_json::json!({"name":"remote","url":"https://example.invalid/mcp","enabled":true})).is_err());
    }
}
