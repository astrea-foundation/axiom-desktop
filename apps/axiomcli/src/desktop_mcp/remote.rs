//! Remote MCP credentials and browser authorization remain in the native client.
use super::{credential_entry, credential_task};
use crate::{AxiomError, Result, config::McpServerConfig, session::SessionStore};
use async_trait::async_trait;
use axiom_acp_extension::{DesktopMcpServer, McpAuthentication, McpRemoteConnection};
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthState,
    StoredCredentials,
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) const BEARER_KEY: &str = "bearer_token";

type PendingAuthorizations = std::sync::Mutex<BTreeMap<String, CancellationToken>>;
static AUTHORIZATIONS: std::sync::OnceLock<PendingAuthorizations> = std::sync::OnceLock::new();

pub(crate) struct AuthorizationGuard(String);
impl Drop for AuthorizationGuard {
    fn drop(&mut self) {
        if let Some(token) = AUTHORIZATIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.0)
        {
            token.cancel();
        }
    }
}
fn authorization_key(store: &SessionStore, name: &str) -> Result<String> {
    Ok(format!("{}:{name}", store.active_desktop_cwd()?.display()))
}
pub(crate) fn begin_authorization(
    store: &SessionStore,
    name: &str,
) -> Result<(CancellationToken, AuthorizationGuard)> {
    let key = authorization_key(store, name)?;
    let token = CancellationToken::new();
    let mut pending = AUTHORIZATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending.contains_key(&key) {
        return Err(AxiomError::InvalidTransition(
            "MCP sign-in is already pending".into(),
        ));
    }
    pending.insert(key.clone(), token.clone());
    Ok((token, AuthorizationGuard(key)))
}
pub(crate) fn cancel_authorization(store: &SessionStore, name: &str) -> Result<()> {
    let key = authorization_key(store, name)?;
    if let Some(token) = AUTHORIZATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        token.cancel();
    }
    Ok(())
}

pub(crate) fn validate_connection(remote: &McpRemoteConnection) -> Result<()> {
    validate_endpoint(&remote.url)?;
    if remote
        .client_id
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control))
    {
        return Err(AxiomError::Config("invalid MCP OAuth client ID".into()));
    }
    if let Some(metadata) = &remote.client_metadata_url {
        let url = validate_endpoint(metadata)?;
        if url.scheme() != "https" || url.path() == "/" {
            return Err(AxiomError::Config(
                "MCP client metadata must be an HTTPS document URL".into(),
            ));
        }
    }
    if remote.auth != McpAuthentication::Oauth
        && (remote.client_id.is_some() || remote.client_metadata_url.is_some())
    {
        return Err(AxiomError::Config(
            "OAuth client settings require browser sign-in".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| AxiomError::Config("invalid remote MCP URL".into()))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if value.len() > 4096
        || value.chars().any(char::is_control)
        || url.host().is_none()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AxiomError::Config("remote MCP URLs must use HTTPS (or loopback HTTP), without credentials, query strings or fragments".into()));
    }
    Ok(url)
}

/// Separate clients never inherit Axiom sign-in headers or follow credential redirects.
pub(crate) fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| AxiomError::Config("could not create the remote MCP HTTP client".into()))
}

pub(crate) async fn auth_manager(remote: &McpRemoteConnection) -> Result<AuthorizationManager> {
    let mut manager = AuthorizationManager::new(&remote.url)
        .await
        .map_err(|_| AxiomError::Config("could not prepare MCP authorization".into()))?;
    manager
        .with_client(http_client()?)
        .map_err(|_| AxiomError::Config("could not prepare MCP authorization transport".into()))?;
    // A configured URL does not authorize guessed /authorize or /token endpoints.
    let resolution = tokio::time::timeout(Duration::from_secs(25), manager.resolve_metadata())
        .await
        .map_err(|_| AxiomError::Config("MCP authorization discovery timed out".into()))?
        .map_err(|_| AxiomError::Config("could not discover MCP authorization metadata".into()))?;
    if !resolution.source.is_discovered() {
        return Err(AxiomError::Config(
            "this MCP server does not publish OAuth discovery metadata".into(),
        ));
    }
    manager.set_metadata(resolution.metadata);
    Ok(manager)
}

#[derive(Clone)]
struct KeychainOAuthStore {
    store: SessionStore,
    name: String,
    credential_id: String,
    remote: McpRemoteConnection,
    staging: bool,
}

impl KeychainOAuthStore {
    fn ensure_current(&self) -> Result<()> {
        if !self.staging
            && !self
                .store
                .desktop_mcp_snapshot(None)?
                .servers
                .iter()
                .any(|s| {
                    s.name == self.name
                        && s.credential_id.as_ref() == Some(&self.credential_id)
                        && s.remote.as_ref() == Some(&self.remote)
                })
        {
            return Err(AxiomError::Storage(
                "MCP connection changed during authorization".into(),
            ));
        }
        Ok(())
    }

    fn entry(&self) -> Result<keyring::Entry> {
        credential_entry(
            &self.store,
            &format!("{}:{}", self.name, self.credential_id),
        )
    }
}

fn credential_error(_: AxiomError) -> AuthError {
    AuthError::InternalError(
        "MCP system credentials are unavailable; reconnect this service in Settings".into(),
    )
}

#[async_trait]
impl CredentialStore for KeychainOAuthStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let bound = self.clone();
        credential_task(move || {
            bound.ensure_current()?;
            match bound.entry()?.get_password() {
                Ok(secret) => serde_json::from_str(&secret)
                    .map(Some)
                    .map_err(|_| AxiomError::Storage("invalid MCP OAuth credentials".into())),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(AxiomError::Storage(
                    "MCP credentials are missing or locked".into(),
                )),
            }
        })
        .await
        .map_err(credential_error)
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let bound = self.clone();
        credential_task(move || {
            bound.ensure_current()?;
            if !bound.staging {
                let previous: StoredCredentials =
                    serde_json::from_str(&bound.entry()?.get_password().map_err(|_| {
                        AxiomError::Storage("MCP credentials are missing or locked".into())
                    })?)?;
                let old_scopes: std::collections::BTreeSet<_> =
                    previous.granted_scopes.iter().collect();
                let new_scopes: std::collections::BTreeSet<_> =
                    credentials.granted_scopes.iter().collect();
                if previous.issuer != credentials.issuer
                    || previous.client_id != credentials.client_id
                    || old_scopes != new_scopes
                {
                    return Err(AxiomError::Storage(
                        "MCP authorization changed; reconnect and review its tools".into(),
                    ));
                }
            }
            bound
                .entry()?
                .set_password(&serde_json::to_string(&credentials)?)
                .map_err(|_| AxiomError::Storage("could not save MCP OAuth credentials".into()))
        })
        .await
        .map_err(credential_error)
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        let bound = self.clone();
        credential_task(move || {
            bound.ensure_current()?;
            match bound.entry()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(AxiomError::Storage(
                    "could not remove MCP credentials".into(),
                )),
            }
        })
        .await
        .map_err(credential_error)
    }
}

pub(crate) fn launch_config(
    store: &SessionStore,
    server: &DesktopMcpServer,
) -> Result<McpServerConfig> {
    let remote = server.remote.as_ref().expect("remote connection");
    validate_connection(remote)?;
    let mut env = BTreeMap::new();
    let mut oauth_credentials: Option<Arc<dyn CredentialStore>> = None;
    match remote.auth {
        McpAuthentication::None => {}
        McpAuthentication::Bearer => {
            let id = server.credential_id.as_ref().ok_or_else(|| {
                AxiomError::Config("save an API token for this connection".into())
            })?;
            let secret = credential_entry(store, &format!("{}:{id}", server.name))?
                .get_password()
                .map_err(|_| AxiomError::Storage("MCP credentials are missing or locked".into()))?;
            env = serde_json::from_str(&secret)
                .map_err(|_| AxiomError::Storage("invalid MCP bearer credentials".into()))?;
            if env.get(BEARER_KEY).is_none_or(String::is_empty) {
                return Err(AxiomError::Storage("MCP bearer token is missing".into()));
            }
        }
        McpAuthentication::Oauth => {
            let id = server.credential_id.as_ref().ok_or_else(|| {
                AxiomError::Config("sign in to this MCP service in Settings".into())
            })?;
            oauth_credentials = Some(Arc::new(KeychainOAuthStore {
                store: store.clone(),
                name: server.name.clone(),
                credential_id: id.clone(),
                remote: remote.clone(),
                staging: false,
            }));
        }
    }
    Ok(McpServerConfig {
        name: server.name.clone(),
        display_name: server.display_name.clone(),
        command: String::new(),
        args: vec![],
        env,
        remote: Some(remote.clone()),
        oauth_credentials,
        oauth_identity: server.credential_id.clone(),
        read_only_tools: vec![],
        tool_timeout_secs: 300,
    })
}

/// Stage a new immutable grant; the caller commits its reference only after success.
pub(crate) async fn authorize(
    store: &SessionStore,
    server: &DesktopMcpServer,
    cancellation: CancellationToken,
) -> Result<String> {
    let remote = server
        .remote
        .as_ref()
        .filter(|r| r.auth == McpAuthentication::Oauth)
        .ok_or_else(|| {
            AxiomError::Config("browser sign-in requires an OAuth remote connection".into())
        })?;
    let credential_id = uuid::Uuid::new_v4().to_string();
    let credentials = KeychainOAuthStore {
        store: store.clone(),
        name: server.name.clone(),
        credential_id: credential_id.clone(),
        remote: remote.clone(),
        staging: true,
    };
    let work = async {
        let mut manager = auth_manager(remote).await?;
        manager.set_credential_store(credentials.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let callback_path = "/mcp/oauth/callback".to_owned();
        let redirect = format!("http://{address}{callback_path}");
        let mut request = AuthorizationRequest::new(&redirect)
            .with_client_name("Axiom")
            .with_application_type("native");
        if let Some(client_id) = &remote.client_id {
            request = request.with_preregistered_client(client_id);
        }
        if let Some(metadata) = &remote.client_metadata_url {
            request = request.with_client_metadata_url(metadata);
        }
        let mut state = OAuthState::Unauthorized(manager);
        state.start_authorization(request).await.map_err(|_| {
            AxiomError::Config(
                "could not start MCP sign-in; check the service's OAuth client registration".into(),
            )
        })?;
        let authorization = state
            .get_authorization_url()
            .await
            .map_err(|_| AxiomError::Config("MCP sign-in URL is unavailable".into()))?;
        let authorization_url = Url::parse(&authorization)
            .map_err(|_| AxiomError::Config("invalid MCP sign-in URL".into()))?;
        let mut endpoint = authorization_url.clone();
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        validate_endpoint(endpoint.as_str())?;
        let expected_state = authorization_url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .ok_or_else(|| AxiomError::Config("MCP sign-in state is missing".into()))?;
        tokio::task::spawn_blocking(move || open::that(authorization))
            .await
            .map_err(|_| AxiomError::Config("could not open the sign-in browser".into()))?
            .map_err(|_| AxiomError::Config("could not open the sign-in browser".into()))?;
        loop {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(5), async {
                let mut chunk = [0; 1024];
                while bytes.len() < 16 * 1024 {
                    let count = socket.read(&mut chunk).await?;
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                std::io::Result::Ok(())
            })
            .await;
            let callback = if matches!(read, Ok(Ok(()))) {
                parse_callback(
                    &bytes,
                    &address.to_string(),
                    &callback_path,
                    &expected_state,
                )
            } else {
                None
            };
            let Some(callback) = callback else {
                let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };
            let result = if let Some(code) = callback.get("code") {
                state
                    .handle_callback_with_issuer(
                        code,
                        &expected_state,
                        callback.get("iss").map(String::as_str),
                    )
                    .await
                    .map_err(|_| {
                        AxiomError::Config(
                            "MCP sign-in callback or token exchange could not be verified".into(),
                        )
                    })
            } else {
                Err(AxiomError::Config("MCP sign-in was declined".into()))
            };
            let body = if result.is_ok() {
                "Connection authorized. You can return to Axiom."
            } else {
                "Connection was not authorized. Return to Axiom and try again."
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            result?;
            return Ok(credential_id.clone());
        }
    };
    let outcome = tokio::select! {
        () = cancellation.cancelled() => Err(AxiomError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(180), work) => result
            .unwrap_or_else(|_| Err(AxiomError::Config("MCP sign-in timed out; connect again when ready".into()))),
    };
    if outcome.is_err() {
        let _ = credentials.clear().await;
    }
    outcome
}

fn parse_callback(
    bytes: &[u8],
    authority: &str,
    path: &str,
    expected_state: &str,
) -> Option<BTreeMap<String, String>> {
    let request = std::str::from_utf8(bytes).ok()?;
    if bytes.len() >= 16 * 1024 || !request.ends_with("\r\n\r\n") {
        return None;
    }
    let mut lines = request.split("\r\n");
    let first: Vec<_> = lines.next()?.split(' ').collect();
    if first.len() != 3 || first[0] != "GET" || first[2] != "HTTP/1.1" || !first[1].starts_with('/')
    {
        return None;
    }
    let mut hosts = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"));
    if hosts.next()?.1.trim() != authority || hosts.next().is_some() {
        return None;
    }
    let url = Url::parse(&format!("http://{authority}{}", first[1])).ok()?;
    if url.path() != path || url.fragment().is_some() {
        return None;
    }
    let mut values = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if values
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return None;
        }
    }
    if values.get("state")?.as_str() != expected_state
        || (values.contains_key("code") == values.contains_key("error"))
    {
        return None;
    }
    Some(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_and_callbacks_bind_to_explicit_destinations() {
        for url in [
            "https://mcp.example/mcp",
            "http://127.0.0.1:1234/mcp",
            "http://[::1]:1234/mcp",
        ] {
            validate_endpoint(url).unwrap();
        }
        for url in [
            "http://example.com/mcp",
            "file:///mcp",
            "https://user:secret@example.com/mcp",
            "https://example.com/mcp?token=secret",
            "https://example.com/mcp#fragment",
        ] {
            assert!(validate_endpoint(url).is_err());
        }
        let valid = b"GET /callback?code=one&state=secret HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n";
        assert!(parse_callback(valid, "127.0.0.1:1234", "/callback", "secret").is_some());
        for bad in [
            "POST /callback?code=one&state=secret HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /callback?code=one&state=wrong HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /callback?code=one&state=secret&state=secret HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /other?code=one&state=secret HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
        ] {
            assert!(
                parse_callback(bad.as_bytes(), "127.0.0.1:1234", "/callback", "secret").is_none()
            );
        }
    }
}
