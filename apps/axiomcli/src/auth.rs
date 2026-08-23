//! Native-client authentication and credential storage.
//!
//! Browser login uses a one-time backend authorization plus PKCE. The browser
//! can notify the loopback listener that approval happened, but it never
//! receives or forwards the permanent API key. Only the initiating CLI can
//! exchange the authorization, and credentials never enter config or journals.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, Url};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, config::Config};

const KEYRING_SERVICE: &str = "stream.axiom.axiomcli";
const KEYRING_ACCOUNT: &str = "inference-api-key";
const CLIENT_ID_FILENAME: &str = "client-id";
const API_KEY_PREFIX: &str = "axm_";
const COMPLETION_CODE_PREFIX: &str = "axc_";
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Environment,
    SystemKeyring,
    ProtectedFile,
}

impl CredentialSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Environment => "AXIOM_API_KEY",
            Self::SystemKeyring => "system keyring",
            Self::ProtectedFile => "protected local file",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountStatus {
    pub key_id: String,
    pub key_name: String,
    pub source: CredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationStatus {
    Missing,
    Valid(AccountStatus),
    Invalid,
    Unavailable(String),
}

trait CredentialStore: Send + Sync {
    fn source(&self) -> CredentialSource;
    fn load(&self) -> std::result::Result<Option<String>, String>;
    fn save(&self, credential: &str) -> std::result::Result<(), String>;
    fn delete(&self) -> std::result::Result<(), String>;

    fn client_id_path(&self) -> Option<PathBuf> {
        client_id_path()
    }
}

#[derive(Debug)]
struct KeyringStore;

impl KeyringStore {
    fn entry() -> std::result::Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
            .map_err(|error| format!("system keyring is unavailable: {error}"))
    }
}

impl CredentialStore for KeyringStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::SystemKeyring
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        match Self::entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("could not read the system keyring: {error}")),
        }
    }

    fn save(&self, credential: &str) -> std::result::Result<(), String> {
        Self::entry()?
            .set_password(credential)
            .map_err(|error| format!("could not write the system keyring: {error}"))
    }

    fn delete(&self) -> std::result::Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("could not clear the system keyring: {error}")),
        }
    }
}

#[derive(Debug)]
struct FileStore {
    path: PathBuf,
}

impl FileStore {
    fn new() -> Result<Self> {
        let directory = Config::data_dir()
            .ok_or_else(|| AxiomError::Storage("platform data directory is unavailable".into()))?
            .join("credentials");
        fs::create_dir_all(&directory)?;
        set_directory_permissions(&directory)?;
        Ok(Self {
            path: directory.join("api-key"),
        })
    }
}

impl CredentialStore for FileStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::ProtectedFile
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        match fs::read_to_string(&self.path) {
            Ok(value) => {
                set_file_permissions(&self.path).map_err(|error| error.to_string())?;
                Ok(Some(value.trim().to_owned()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read credential file: {error}")),
        }
    }

    fn save(&self, credential: &str) -> std::result::Result<(), String> {
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&self.path)
            .map_err(|error| format!("could not open credential file: {error}"))?;
        file.write_all(credential.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not write credential file: {error}"))?;
        set_file_permissions(&self.path).map_err(|error| error.to_string())
    }

    fn delete(&self) -> std::result::Result<(), String> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove credential file: {error}")),
        }
    }
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

struct AuthState {
    credential: Option<SecretString>,
    source: CredentialSource,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthState")
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .field("source", &self.source)
            .finish()
    }
}

#[derive(Clone)]
pub struct AuthManager {
    state: Arc<RwLock<AuthState>>,
    store: Arc<dyn CredentialStore>,
    client: Client,
    api_base_url: Url,
    web_base_url: Url,
    launch_browser: bool,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthManager")
            .field("state", &self.state)
            .field("api_base_url", &self.api_base_url)
            .field("web_base_url", &self.web_base_url)
            .field("launch_browser", &self.launch_browser)
            .finish_non_exhaustive()
    }
}

impl AuthManager {
    pub fn new(api_base_url: &str, timeout: Duration) -> Result<Self> {
        let api_base_url = parse_service_url(api_base_url, "API")?;
        let web_url =
            std::env::var("AXIOM_WEB_URL").unwrap_or_else(|_| "https://axiom.stream".to_owned());
        let web_base_url = parse_service_url(&web_url, "website")?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AxiomError::Config(format!("could not build auth client: {error}")))?;

        if let Ok(value) = std::env::var("AXIOM_API_KEY") {
            validate_api_key_shape(&value)?;
            let store: Arc<dyn CredentialStore> = Arc::new(EnvironmentStore);
            return Ok(Self {
                state: Arc::new(RwLock::new(AuthState {
                    credential: Some(SecretString::from(value)),
                    source: CredentialSource::Environment,
                })),
                store,
                client,
                api_base_url,
                web_base_url,
                launch_browser: true,
            });
        }

        let requested_store = std::env::var("AXIOMCLI_CREDENTIAL_STORE").ok();
        let store: Arc<dyn CredentialStore> = match requested_store.as_deref() {
            Some("file") => Arc::new(FileStore::new()?),
            Some("keyring") | None => {
                let keyring: Arc<dyn CredentialStore> = Arc::new(KeyringStore);
                match keyring.load() {
                    Ok(_) => keyring,
                    Err(error) if requested_store.is_some() => {
                        return Err(AxiomError::Storage(error));
                    }
                    Err(_) => Arc::new(FileStore::new()?),
                }
            }
            Some(other) => {
                return Err(AxiomError::Config(format!(
                    "unknown AXIOMCLI_CREDENTIAL_STORE `{other}`; use keyring or file"
                )));
            }
        };
        let credential = store
            .load()
            .map_err(AxiomError::Storage)?
            .filter(|value| validate_api_key_shape(value).is_ok())
            .map(SecretString::from);
        let source = store.source();
        Ok(Self {
            state: Arc::new(RwLock::new(AuthState { credential, source })),
            store,
            client,
            api_base_url,
            web_base_url,
            launch_browser: true,
        })
    }

    #[must_use]
    pub fn has_credential(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .credential
            .is_some()
    }

    #[must_use]
    pub fn credential_source(&self) -> CredentialSource {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .source
    }

    pub fn credential(&self) -> Result<String> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .credential
            .as_ref()
            .map(|value| value.expose_secret().to_owned())
            .ok_or_else(|| {
                AxiomError::Provider(
                    "authentication required; use /login to connect your Axiom account".into(),
                )
            })
    }

    pub fn save_pasted_key(&self, value: &str) -> Result<()> {
        let value = value.trim();
        validate_api_key_shape(value)?;
        self.save_credential(value)
    }

    fn save_credential(&self, value: &str) -> Result<()> {
        validate_api_key_shape(value)?;
        self.store.save(value).map_err(AxiomError::Storage)?;
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.credential = Some(SecretString::from(value.to_owned()));
        state.source = self.store.source();
        Ok(())
    }

    pub fn logout(&self) -> Result<()> {
        self.store.delete().map_err(AxiomError::Storage)?;
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .credential = None;
        Ok(())
    }

    pub async fn validate(&self) -> ValidationStatus {
        let Ok(credential) = self.credential() else {
            return ValidationStatus::Missing;
        };
        let response = self
            .client
            .get(self.endpoint("/api/v1/auth/api-key"))
            .bearer_auth(&credential)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return ValidationStatus::Unavailable(format!(
                    "could not reach Axiom to validate this login: {error}"
                ));
            }
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            self.clear_if_current(&credential);
            return ValidationStatus::Invalid;
        }
        if !response.status().is_success() {
            return ValidationStatus::Unavailable(format!(
                "Axiom credential validation returned HTTP {}",
                response.status()
            ));
        }
        match response.json::<ApiKeyStatusResponse>().await {
            Ok(value) => ValidationStatus::Valid(AccountStatus {
                key_id: value.id,
                key_name: value.name,
                source: self.credential_source(),
            }),
            Err(_) => ValidationStatus::Unavailable(
                "Axiom returned an invalid credential-validation response".into(),
            ),
        }
    }

    pub async fn start_browser_login(&self) -> Result<BrowserLogin> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let verifier = random_urlsafe(32)?;
        let state = random_urlsafe(32)?;
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let callback_uri = format!("http://127.0.0.1:{port}/callback");
        let client_id_path = self
            .store
            .client_id_path()
            .ok_or_else(|| AxiomError::Storage("platform data directory is unavailable".into()))?;
        let client_id = load_or_create_client_id(&client_id_path)?;
        let client_name = client_name(&client_id);
        let response = self
            .client
            .post(self.endpoint("/api/v1/auth/cli/authorizations"))
            .json(&CreateAuthorizationRequest {
                code_challenge,
                callback_uri,
                client_id,
                client_name,
            })
            .send()
            .await
            .map_err(|error| AxiomError::Provider(format!("could not start login: {error}")))?;
        if !response.status().is_success() {
            return Err(AxiomError::Provider(format!(
                "Axiom rejected the login request with HTTP {}",
                response.status()
            )));
        }
        let created = response
            .json::<CreatedAuthorization>()
            .await
            .map_err(|_| AxiomError::Protocol("invalid login response from Axiom".into()))?;
        let mut authorization_url = self
            .web_base_url
            .join("/cli/authorize")
            .map_err(|_| AxiomError::Config("invalid website authorization URL".into()))?;
        authorization_url
            .query_pairs_mut()
            .append_pair("id", &created.id)
            .append_pair("state", &state);
        let browser_opened =
            self.launch_browser && open::that_detached(authorization_url.as_str()).is_ok();
        Ok(BrowserLogin {
            manager: self.clone(),
            listener,
            authorization_id: created.id,
            device_code: SecretString::from(created.device_code),
            verifier: SecretString::from(verifier),
            state: SecretString::from(state),
            user_code: created.user_code,
            authorization_url,
            expires_at: created.expires_at,
            interval: Duration::from_secs(created.interval_seconds.clamp(1, 10)),
            browser_opened,
        })
    }

    pub async fn complete_with_code(&self, completion_code: &str) -> Result<AccountStatus> {
        let completion_code = completion_code.trim();
        let authorization_id = completion_code
            .strip_prefix(COMPLETION_CODE_PREFIX)
            .and_then(|value| value.split_once('.'))
            .map(|(id, _)| id)
            .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| AxiomError::Config("invalid one-time authorization code".into()))?;
        let response = self
            .exchange(ExchangeRequest {
                authorization_id,
                device_code: None,
                code_verifier: None,
                completion_code: Some(completion_code),
            })
            .await?;
        self.finish_exchange(response)
    }

    async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<ExchangeResponse> {
        let response = self
            .client
            .post(self.endpoint("/api/v1/auth/cli/exchange"))
            .json(&request)
            .send()
            .await
            .map_err(|error| AxiomError::Provider(format!("login exchange failed: {error}")))?;
        if !response.status().is_success() {
            return Err(AxiomError::Provider(format!(
                "Axiom rejected the one-time login with HTTP {}",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|_| AxiomError::Protocol("invalid login exchange response".into()))
    }

    fn finish_exchange(&self, response: ExchangeResponse) -> Result<AccountStatus> {
        if response.status != "complete" {
            return Err(AxiomError::InvalidTransition(
                "authorization has not been approved yet".into(),
            ));
        }
        let token = response
            .token
            .ok_or_else(|| AxiomError::Protocol("completed login omitted its credential".into()))?;
        let key = response
            .key
            .ok_or_else(|| AxiomError::Protocol("completed login omitted key metadata".into()))?;
        self.save_credential(&token)?;
        Ok(AccountStatus {
            key_id: key.id,
            key_name: key.name,
            source: self.credential_source(),
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        self.api_base_url
            .join(path)
            .expect("static API path is valid")
    }

    fn clear_if_current(&self, checked: &str) {
        let is_current = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .credential
            .as_ref()
            .is_some_and(|value| value.expose_secret() == checked);
        if is_current {
            let _ = self.logout();
        }
    }
}

#[derive(Debug)]
struct EnvironmentStore;

impl CredentialStore for EnvironmentStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::Environment
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }

    fn save(&self, _credential: &str) -> std::result::Result<(), String> {
        Err("AXIOM_API_KEY overrides persisted login; unset it before signing in".into())
    }

    fn delete(&self) -> std::result::Result<(), String> {
        Err("AXIOM_API_KEY is controlled by the process environment; unset it to sign out".into())
    }
}

pub struct BrowserLogin {
    manager: AuthManager,
    listener: TcpListener,
    authorization_id: String,
    device_code: SecretString,
    verifier: SecretString,
    state: SecretString,
    user_code: String,
    authorization_url: Url,
    expires_at: String,
    interval: Duration,
    browser_opened: bool,
}

impl std::fmt::Debug for BrowserLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserLogin")
            .field("authorization_id", &self.authorization_id)
            .field("device_code", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field("user_code", &self.user_code)
            .field("authorization_url", &self.authorization_url)
            .field("expires_at", &self.expires_at)
            .field("browser_opened", &self.browser_opened)
            .finish_non_exhaustive()
    }
}

impl BrowserLogin {
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    #[must_use]
    pub fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }

    #[must_use]
    pub const fn browser_opened(&self) -> bool {
        self.browser_opened
    }

    pub async fn complete(self, cancellation: CancellationToken) -> Result<AccountStatus> {
        let deadline = DateTime::parse_from_rfc3339(&self.expires_at).map_or_else(
            |_| Utc::now() + chrono::Duration::minutes(10),
            |value| value.with_timezone(&Utc),
        );
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            if Utc::now() >= deadline {
                return Err(AxiomError::Provider(
                    "browser authorization expired; run /login to try again".into(),
                ));
            }
            tokio::select! {
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                _ = ticker.tick() => {
                    if let Some(account) = self.poll_exchange().await? {
                        return Ok(account);
                    }
                }
                accepted = self.listener.accept() => {
                    let (mut socket, _) = accepted?;
                    if handle_loopback_request(
                        &mut socket,
                        self.manager.web_base_url.origin().ascii_serialization().as_str(),
                        &self.authorization_id,
                        self.state.expose_secret(),
                    ).await?
                        && let Some(account) = self.poll_exchange().await?
                    {
                        return Ok(account);
                    }
                }
            }
        }
    }

    async fn poll_exchange(&self) -> Result<Option<AccountStatus>> {
        let response = self
            .manager
            .exchange(ExchangeRequest {
                authorization_id: &self.authorization_id,
                device_code: Some(self.device_code.expose_secret()),
                code_verifier: Some(self.verifier.expose_secret()),
                completion_code: None,
            })
            .await?;
        if response.status == "pending" {
            return Ok(None);
        }
        self.manager.finish_exchange(response).map(Some)
    }
}

async fn handle_loopback_request(
    socket: &mut TcpStream,
    allowed_origin: &str,
    authorization_id: &str,
    expected_state: &str,
) -> Result<bool> {
    let mut request = vec![0_u8; MAX_HTTP_REQUEST_BYTES];
    let size = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut request))
        .await
        .map_err(|_| AxiomError::Protocol("loopback callback timed out".into()))??;
    if size == 0 || size == request.len() {
        write_loopback_response(socket, StatusCode::BAD_REQUEST, allowed_origin, false).await?;
        return Ok(false);
    }
    let request = String::from_utf8_lossy(&request[..size]);
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let origin = lines.find_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("origin"))
            .map(|(_, value)| value.trim())
    });
    if request_line.starts_with("OPTIONS ") {
        let allowed = origin == Some(allowed_origin);
        write_loopback_response(
            socket,
            if allowed {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::FORBIDDEN
            },
            allowed_origin,
            allowed,
        )
        .await?;
        return Ok(false);
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let parsed = Url::parse(&format!("http://127.0.0.1{target}"));
    let valid = origin == Some(allowed_origin)
        && parsed.as_ref().is_ok_and(|url| {
            url.path() == "/callback"
                && url
                    .query_pairs()
                    .any(|(name, value)| name == "authorization_id" && value == authorization_id)
                && url
                    .query_pairs()
                    .any(|(name, value)| name == "state" && value == expected_state)
        });
    write_loopback_response(
        socket,
        if valid {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        },
        allowed_origin,
        valid,
    )
    .await?;
    Ok(valid)
}

async fn write_loopback_response(
    socket: &mut TcpStream,
    status: StatusCode,
    allowed_origin: &str,
    include_cors: bool,
) -> Result<()> {
    let reason = status.canonical_reason().unwrap_or("Response");
    let body = if status.is_success() {
        "AxiomCLI received the approval. Return to your terminal."
    } else {
        "This AxiomCLI callback was rejected."
    };
    let cors = if include_cors {
        format!(
            "Access-Control-Allow-Origin: {allowed_origin}\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Private-Network: true\r\nVary: Origin\r\n"
        )
    } else {
        String::new()
    };
    let response = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\n{cors}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        status.as_u16(),
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

fn random_urlsafe(size: usize) -> Result<String> {
    let mut bytes = vec![0_u8; size];
    getrandom::fill(&mut bytes)
        .map_err(|error| AxiomError::Storage(format!("secure randomness unavailable: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn client_id_path() -> Option<PathBuf> {
    Config::data_dir().map(|directory| directory.join("identity").join(CLIENT_ID_FILENAME))
}

fn load_or_create_client_id(path: &Path) -> Result<String> {
    match read_client_id(path) {
        Ok(client_id) => return Ok(client_id),
        Err(AxiomError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let directory = path.parent().ok_or_else(|| {
        AxiomError::Storage("local AxiomCLI installation identity path is invalid".into())
    })?;
    fs::create_dir_all(directory)?;
    set_directory_permissions(directory)?;

    let client_id = random_urlsafe(32)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(client_id.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            set_file_permissions(path)?;
            Ok(client_id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_client_id(path),
        Err(error) => Err(error.into()),
    }
}

fn read_client_id(path: &Path) -> Result<String> {
    let client_id = fs::read_to_string(path)?;
    set_file_permissions(path)?;
    let client_id = client_id.trim();
    if !(32..=128).contains(&client_id.len())
        || !client_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AxiomError::Storage(format!(
            "local AxiomCLI installation identity at {} is invalid",
            path.display()
        )));
    }
    Ok(client_id.to_owned())
}

fn client_name(client_id: &str) -> String {
    let hostname = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .and_then(|value| sanitize_device_label(&value))
        .unwrap_or_else(|| format!("{} device", std::env::consts::OS));
    let suffix = &client_id[client_id.len() - 6..];
    format!("{hostname} · {suffix}")
}

fn sanitize_device_label(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| !character.is_control())
        .take(71)
        .collect::<String>();
    let normalized = normalized.trim();
    (!normalized.is_empty()).then(|| normalized.to_owned())
}

fn validate_api_key_shape(value: &str) -> Result<()> {
    if !value.starts_with(API_KEY_PREFIX)
        || !(16..=512).contains(&value.len())
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(AxiomError::Config(
            "invalid Axiom authorization key; expected an axm_ key".into(),
        ));
    }
    Ok(())
}

fn parse_service_url(value: &str, label: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| AxiomError::Config(format!("invalid {label} URL: {error}")))?;
    let loopback_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if (url.scheme() != "https" && !loopback_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AxiomError::Config(format!(
            "{label} URL must use HTTPS (or loopback HTTP) and contain no credentials"
        )));
    }
    Ok(url)
}

#[derive(Serialize)]
struct CreateAuthorizationRequest {
    code_challenge: String,
    callback_uri: String,
    client_id: String,
    client_name: String,
}

#[derive(Deserialize)]
struct CreatedAuthorization {
    id: String,
    device_code: String,
    user_code: String,
    expires_at: String,
    interval_seconds: u64,
}

#[derive(Serialize)]
struct ExchangeRequest<'a> {
    authorization_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_code: Option<&'a str>,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    status: String,
    token: Option<String>,
    key: Option<ApiKeyResponse>,
}

#[derive(Deserialize)]
struct ApiKeyResponse {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct ApiKeyStatusResponse {
    id: String,
    name: String,
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode as AxumStatus},
        routing::{get, post},
    };
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug)]
    struct MemoryStore {
        credential: Mutex<Option<String>>,
        client_id_path: PathBuf,
    }

    impl CredentialStore for MemoryStore {
        fn source(&self) -> CredentialSource {
            CredentialSource::ProtectedFile
        }

        fn load(&self) -> std::result::Result<Option<String>, String> {
            Ok(self.credential.lock().expect("credential lock").clone())
        }

        fn save(&self, credential: &str) -> std::result::Result<(), String> {
            *self.credential.lock().expect("credential lock") = Some(credential.to_owned());
            Ok(())
        }

        fn delete(&self) -> std::result::Result<(), String> {
            *self.credential.lock().expect("credential lock") = None;
            Ok(())
        }

        fn client_id_path(&self) -> Option<PathBuf> {
            Some(self.client_id_path.clone())
        }
    }

    #[derive(Debug)]
    struct MockAuthorizationState {
        challenge: Mutex<Option<String>>,
        client_ids: Mutex<Vec<String>>,
        client_names: Mutex<Vec<String>>,
        approved: AtomicBool,
        identity_directory: tempfile::TempDir,
    }

    impl Default for MockAuthorizationState {
        fn default() -> Self {
            Self {
                challenge: Mutex::new(None),
                client_ids: Mutex::new(Vec::new()),
                client_names: Mutex::new(Vec::new()),
                approved: AtomicBool::new(false),
                identity_directory: tempfile::tempdir().expect("identity temp directory"),
            }
        }
    }

    async fn create_authorization(
        State(state): State<Arc<MockAuthorizationState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        *state.challenge.lock().expect("challenge lock") =
            body["code_challenge"].as_str().map(str::to_owned);
        assert!(
            body["callback_uri"]
                .as_str()
                .is_some_and(|value| value.starts_with("http://127.0.0.1:"))
        );
        state
            .client_ids
            .lock()
            .expect("client ID lock")
            .push(body["client_id"].as_str().expect("client ID").to_owned());
        state.client_names.lock().expect("client name lock").push(
            body["client_name"]
                .as_str()
                .expect("client name")
                .to_owned(),
        );
        Json(json!({
            "id": "0123456789abcdef0123456789abcdef",
            "device_code": "device-code-with-enough-entropy-for-the-test",
            "user_code": "BLOM-1234",
            "expires_at": (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
            "interval_seconds": 1
        }))
    }

    async fn exchange_authorization(
        State(state): State<Arc<MockAuthorizationState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if let Some(verifier) = body["code_verifier"].as_str() {
            let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            assert_eq!(
                state.challenge.lock().expect("challenge lock").as_deref(),
                Some(actual.as_str())
            );
        } else {
            assert!(
                body["completion_code"]
                    .as_str()
                    .is_some_and(|value| value.starts_with("axc_0123456789abcdef0123456789abcdef."))
            );
        }
        if !state.approved.load(Ordering::Acquire) {
            return Json(json!({ "status": "pending", "token": null, "key": null }));
        }
        Json(json!({
            "status": "complete",
            "token": "axm_mock-browser-exchange-token",
            "key": { "id": "key-id", "name": "AxiomCLI", "used": false }
        }))
    }

    async fn validate_authorization(headers: HeaderMap) -> (AxumStatus, Json<Value>) {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer axm_mock-browser-exchange-token")
        {
            return (
                AxumStatus::OK,
                Json(json!({ "id": "key-id", "name": "AxiomCLI" })),
            );
        }
        (
            AxumStatus::UNAUTHORIZED,
            Json(json!({ "detail": "invalid API key" })),
        )
    }

    async fn mock_manager() -> (AuthManager, Arc<MockAuthorizationState>) {
        let state = Arc::new(MockAuthorizationState::default());
        let app = Router::new()
            .route(
                "/api/v1/auth/cli/authorizations",
                post(create_authorization),
            )
            .route("/api/v1/auth/cli/exchange", post(exchange_authorization))
            .route("/api/v1/auth/api-key", get(validate_authorization))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock auth server");
        });
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            credential: Mutex::new(None),
            client_id_path: state.identity_directory.path().join(CLIENT_ID_FILENAME),
        });
        let manager = AuthManager {
            state: Arc::new(RwLock::new(AuthState {
                credential: None,
                source: store.source(),
            })),
            store,
            client: Client::builder().build().expect("auth client"),
            api_base_url: Url::parse(&base).expect("API URL"),
            web_base_url: Url::parse("https://axiom.stream").expect("web URL"),
            launch_browser: false,
        };
        (manager, state)
    }

    #[test]
    fn api_keys_and_service_urls_are_validated_without_echoing_secrets() {
        assert!(validate_api_key_shape("axm_1234567890123456").is_ok());
        assert!(validate_api_key_shape("not-a-key").is_err());
        assert!(parse_service_url("https://api.axiom.stream", "API").is_ok());
        assert!(parse_service_url("http://127.0.0.1:8000", "API").is_ok());
        assert!(parse_service_url("http://example.com", "API").is_err());
    }

    #[test]
    fn pkce_values_have_the_required_entropy_and_wire_shape() {
        let verifier = random_urlsafe(32).expect("random verifier");
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
        assert_ne!(verifier, random_urlsafe(32).expect("second verifier"));
    }

    #[test]
    fn installation_identity_is_stable_and_has_a_distinct_device_label() {
        let directory = tempfile::tempdir().expect("identity temp directory");
        let path = directory.path().join(CLIENT_ID_FILENAME);
        let first = load_or_create_client_id(&path).expect("create client ID");
        let second = load_or_create_client_id(&path).expect("reload client ID");

        assert_eq!(first, second);
        assert_eq!(first.len(), 43);
        let name = client_name(&first);
        assert!(name.ends_with(&first[first.len() - 6..]));
        assert!(name.chars().count() <= 80);
    }

    #[test]
    fn browser_login_debug_output_redacts_exchange_secrets() {
        let text = format!(
            "{:?}",
            AuthState {
                credential: Some(SecretString::from("axm_super-secret".to_owned())),
                source: CredentialSource::ProtectedFile,
            }
        );
        assert!(!text.contains("super-secret"));
        assert!(text.contains("REDACTED"));
    }

    #[tokio::test]
    async fn browser_login_uses_pkce_polls_and_persists_only_after_approval() {
        let (manager, state) = mock_manager().await;
        let login = manager.start_browser_login().await.expect("start login");
        assert_eq!(login.user_code(), "BLOM-1234");
        assert!(login.authorization_url().contains("/cli/authorize?"));
        assert!(!login.browser_opened());
        assert!(!manager.has_credential());
        let first_client_id = state
            .client_ids
            .lock()
            .expect("client ID lock")
            .first()
            .expect("client ID request")
            .clone();
        let first_client_name = state
            .client_names
            .lock()
            .expect("client name lock")
            .first()
            .expect("client name request")
            .clone();
        assert!(first_client_name.ends_with(&first_client_id[first_client_id.len() - 6..]));

        state.approved.store(true, Ordering::Release);
        let account = tokio::time::timeout(
            Duration::from_secs(2),
            login.complete(CancellationToken::new()),
        )
        .await
        .expect("login timeout")
        .expect("complete login");
        assert_eq!(account.key_id, "key-id");
        assert_eq!(
            manager.credential().expect("saved key"),
            "axm_mock-browser-exchange-token"
        );
        assert!(matches!(
            manager.validate().await,
            ValidationStatus::Valid(_)
        ));
    }

    #[tokio::test]
    async fn one_time_browser_code_can_finish_login_after_a_restart() {
        let (manager, state) = mock_manager().await;
        state.approved.store(true, Ordering::Release);

        let account = manager
            .complete_with_code("axc_0123456789abcdef0123456789abcdef.synthetic-completion-secret")
            .await
            .expect("complete fallback login");

        assert_eq!(account.key_name, "AxiomCLI");
        assert_eq!(
            manager.credential().expect("saved credential"),
            "axm_mock-browser-exchange-token"
        );
    }

    #[tokio::test]
    async fn rejected_persisted_credentials_are_cleared() {
        let (manager, _) = mock_manager().await;
        manager
            .save_pasted_key("axm_1234567890123456")
            .expect("save synthetic key");

        assert_eq!(manager.validate().await, ValidationStatus::Invalid);
        assert!(!manager.has_credential());
        assert!(manager.store.load().expect("load credential").is_none());
    }

    async fn send_loopback_request(request: &str) -> (bool, String) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("loopback address");
        let request = request.to_owned();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.expect("connect callback");
            stream
                .write_all(request.as_bytes())
                .await
                .expect("write callback");
            stream.shutdown().await.expect("finish callback request");
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .await
                .expect("read callback response");
            response
        });
        let (mut socket, _) = listener.accept().await.expect("accept callback");
        let accepted = handle_loopback_request(
            &mut socket,
            "https://axiom.stream",
            "0123456789abcdef0123456789abcdef",
            "expected-state",
        )
        .await
        .expect("handle callback");
        drop(socket);
        (accepted, client.await.expect("callback client"))
    }

    #[tokio::test]
    async fn loopback_callback_requires_the_exact_origin_id_and_state() {
        let valid = concat!(
            "GET /callback?authorization_id=0123456789abcdef0123456789abcdef&state=expected-state HTTP/1.1\r\n",
            "Host: 127.0.0.1\r\n",
            "Origin: https://axiom.stream\r\n\r\n",
        );
        let (accepted, response) = send_loopback_request(valid).await;
        assert!(accepted);
        assert!(response.starts_with("HTTP/1.1 200"));

        for invalid in [
            valid.replace("expected-state", "wrong-state"),
            valid.replace("https://axiom.stream", "https://evil.example"),
            valid.replace(
                "0123456789abcdef0123456789abcdef",
                "fedcba9876543210fedcba9876543210",
            ),
        ] {
            let (accepted, response) = send_loopback_request(&invalid).await;
            assert!(!accepted);
            assert!(response.starts_with("HTTP/1.1 400"));
        }
    }
}
