//! Native-client authentication and credential storage.
//!
//! Browser login uses a one-time backend authorization plus PKCE. The browser
//! can notify the loopback listener that approval happened, while PKCE polling
//! also completes when that callback is blocked. Only the initiating CLI can
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
use fs2::FileExt as _;
use reqwest::{Client, StatusCode, Url};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::Mutex as AsyncMutex,
};
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result};

const KEYRING_SERVICE: &str = "stream.axiom.axiomcli";
const KEYRING_ACCOUNT: &str = "inference-api-key";
const API_KEY_PREFIX: &str = "axm_";
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;
const CREDENTIAL_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(3);

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
    generation: u64,
    revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationStatus {
    Missing,
    Valid(AccountStatus),
    Invalid,
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedValidationStatus {
    pub status: ValidationStatus,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthVersion {
    generation: u64,
    revision: u64,
}

trait CredentialStore: Send + Sync {
    fn source(&self) -> CredentialSource;
    fn load(&self) -> std::result::Result<Option<String>, String>;
    fn save(&self, credential: &str) -> std::result::Result<(), String>;
    fn delete(&self) -> std::result::Result<(), String>;
    fn client_id_path(&self) -> &Path;
}

#[derive(Debug)]
struct KeyringStore {
    client_id_path: PathBuf,
}

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

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
    }
}

#[derive(Debug)]
struct FileStore {
    path: PathBuf,
    client_id_path: PathBuf,
}

impl FileStore {
    fn new(directory: &Path, client_id_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(directory)?;
        set_directory_permissions(directory)?;
        Ok(Self {
            path: directory.join("api-key"),
            client_id_path,
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

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
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
    generation: u64,
    revision: u64,
    unavailable_detail: Option<String>,
}

struct CredentialSnapshot {
    credential: String,
    source: CredentialSource,
    generation: u64,
}

struct AuthShared {
    state: RwLock<AuthState>,
    operations: Arc<AsyncMutex<()>>,
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
            .field("generation", &self.generation)
            .field("revision", &self.revision)
            .field("unavailable_detail", &self.unavailable_detail)
            .finish()
    }
}

#[derive(Clone)]
pub struct AuthManager {
    shared: Arc<AuthShared>,
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
            .field("state", &self.shared.state)
            .field("api_base_url", &self.api_base_url)
            .field("web_base_url", &self.web_base_url)
            .field("launch_browser", &self.launch_browser)
            .finish_non_exhaustive()
    }
}

impl AuthManager {
    pub fn new(api_base_url: &str, timeout: Duration) -> Result<Self> {
        let paths = crate::paths::AxiomPaths::discover()?;
        Self::new_with_paths(api_base_url, timeout, &paths)
    }

    pub async fn new_with_paths_async(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
    ) -> Result<Self> {
        let owned_api_base_url = api_base_url.to_owned();
        let owned_paths = paths.clone();
        let initialization = tokio::task::spawn_blocking(move || {
            Self::new_with_paths(&owned_api_base_url, timeout, &owned_paths)
        });
        match tokio::time::timeout(CREDENTIAL_INITIALIZATION_TIMEOUT, initialization).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(AxiomError::Storage(format!(
                "credential initialization task failed: {error}"
            ))),
            Err(_) => Self::new_unavailable(
                api_base_url,
                timeout,
                paths,
                "the system credential store did not respond during startup; Axiom is running signed out",
            ),
        }
    }

    fn new_unavailable(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
        detail: &str,
    ) -> Result<Self> {
        let api_base_url = parse_service_url(api_base_url, "API")?;
        let web_url =
            std::env::var("AXIOM_WEB_URL").unwrap_or_else(|_| "https://axiom.stream".to_owned());
        let web_base_url = parse_service_url(&web_url, "website")?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AxiomError::Config(format!("could not build auth client: {error}")))?;
        let detail = detail.to_owned();
        let store: Arc<dyn CredentialStore> = Arc::new(UnavailableStore {
            detail: detail.clone(),
            client_id_path: paths.identity_path(),
        });
        Ok(Self {
            shared: Arc::new(AuthShared {
                state: RwLock::new(AuthState {
                    credential: None,
                    source: store.source(),
                    generation: 0,
                    revision: 0,
                    unavailable_detail: Some(detail),
                }),
                operations: Arc::new(AsyncMutex::new(())),
            }),
            store,
            client,
            api_base_url,
            web_base_url,
            launch_browser: true,
        })
    }

    pub fn new_with_paths(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
    ) -> Result<Self> {
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
            let store: Arc<dyn CredentialStore> = Arc::new(EnvironmentStore {
                client_id_path: paths.identity_path(),
            });
            return Ok(Self {
                shared: Arc::new(AuthShared {
                    state: RwLock::new(AuthState {
                        credential: Some(SecretString::from(value)),
                        source: CredentialSource::Environment,
                        generation: 0,
                        revision: 0,
                        unavailable_detail: None,
                    }),
                    operations: Arc::new(AsyncMutex::new(())),
                }),
                store,
                client,
                api_base_url,
                web_base_url,
                launch_browser: true,
            });
        }

        let requested_store = std::env::var("AXIOMCLI_CREDENTIAL_STORE").ok();
        let (store, loaded_credential, unavailable_detail): (
            Arc<dyn CredentialStore>,
            Option<String>,
            Option<String>,
        ) = match requested_store.as_deref() {
            Some("file") => {
                let store: Arc<dyn CredentialStore> = Arc::new(FileStore::new(
                    &paths.credentials_dir(),
                    paths.identity_path(),
                )?);
                let credential = store.load().map_err(AxiomError::Storage)?;
                (store, credential, None)
            }
            Some("keyring") | None => {
                let keyring: Arc<dyn CredentialStore> = Arc::new(KeyringStore {
                    client_id_path: paths.identity_path(),
                });
                match keyring.load() {
                    Ok(credential) => (keyring, credential, None),
                    Err(error) if requested_store.is_some() || cfg!(windows) => {
                        let detail = format!(
                            "the system credential store is unavailable: {error}; sign-in remains disabled until it is restored (set AXIOMCLI_CREDENTIAL_STORE=file to opt in to protected file storage)"
                        );
                        let store: Arc<dyn CredentialStore> = Arc::new(UnavailableStore {
                            detail: detail.clone(),
                            client_id_path: paths.identity_path(),
                        });
                        (store, None, Some(detail))
                    }
                    Err(_) => {
                        let store: Arc<dyn CredentialStore> = Arc::new(FileStore::new(
                            &paths.credentials_dir(),
                            paths.identity_path(),
                        )?);
                        let credential = store.load().map_err(AxiomError::Storage)?;
                        (store, credential, None)
                    }
                }
            }
            Some(other) => {
                return Err(AxiomError::Config(format!(
                    "unknown AXIOMCLI_CREDENTIAL_STORE `{other}`; use keyring or file"
                )));
            }
        };
        let credential = loaded_credential
            .filter(|value| validate_api_key_shape(value).is_ok())
            .map(SecretString::from);
        let source = store.source();
        Ok(Self {
            shared: Arc::new(AuthShared {
                state: RwLock::new(AuthState {
                    credential,
                    source,
                    generation: 0,
                    revision: 0,
                    unavailable_detail,
                }),
                operations: Arc::new(AsyncMutex::new(())),
            }),
            store,
            client,
            api_base_url,
            web_base_url,
            launch_browser: true,
        })
    }

    #[must_use]
    pub fn has_credential(&self) -> bool {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .credential
            .is_some()
    }

    #[must_use]
    pub fn credential_source(&self) -> CredentialSource {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .source
    }

    pub fn credential(&self) -> Result<String> {
        self.shared
            .state
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

    #[must_use]
    pub fn account_status_is_current(&self, status: &AccountStatus) -> bool {
        self.generation() == status.generation && self.has_credential()
    }

    #[must_use]
    pub const fn account_status_revision(status: &AccountStatus) -> u64 {
        status.revision
    }

    pub async fn save_pasted_key_async(&self, value: &str) -> Result<()> {
        let value = value.trim();
        validate_api_key_shape(value)?;
        self.save_credential_async(value, None, None)
            .await
            .map(|_| ())
    }

    pub async fn save_pasted_key_with_cancellation(
        &self,
        value: &str,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        let value = value.trim();
        validate_api_key_shape(value)?;
        self.save_credential_async(value, None, Some(cancellation))
            .await
            .map(|version| version.revision)
    }

    async fn save_credential_async(
        &self,
        value: &str,
        expected_generation: Option<u64>,
        cancellation: Option<&CancellationToken>,
    ) -> Result<AuthVersion> {
        validate_api_key_shape(value)?;
        let operation_future = Arc::clone(&self.shared.operations).lock_owned();
        let operation = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                operation = operation_future => operation,
            }
        } else {
            operation_future.await
        };
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(AxiomError::Cancelled);
        }
        let store = Arc::clone(&self.store);
        let shared = Arc::clone(&self.shared);
        let source = store.source();
        let value = value.to_owned();
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            if let Some(expected_generation) = expected_generation {
                let current_generation = shared
                    .state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .generation;
                if current_generation != expected_generation {
                    return Err(AxiomError::InvalidTransition(
                        "Axiom authorization changed while browser sign-in was pending; start sign-in again"
                            .into(),
                    ));
                }
            }

            store.save(&value).map_err(AxiomError::Storage)?;

            // Persistence is the commit point: in-memory readers only see the
            // new credential after the backing store has accepted it. Keeping
            // the commit in this blocking task makes cancellation safe.
            let mut state = shared
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.credential = Some(SecretString::from(value));
            state.source = source;
            state.unavailable_detail = None;
            state.generation = state.generation.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
            Ok(AuthVersion {
                generation: state.generation,
                revision: state.revision,
            })
        })
            .await
            .map_err(|error| {
                AxiomError::Storage(format!("credential persistence task failed: {error}"))
            })?
    }

    pub async fn logout_async(&self) -> Result<()> {
        self.logout_inner(None).await.map(|_| ())
    }

    pub async fn logout_with_cancellation(&self, cancellation: &CancellationToken) -> Result<u64> {
        self.logout_inner(Some(cancellation)).await
    }

    async fn logout_inner(&self, cancellation: Option<&CancellationToken>) -> Result<u64> {
        let operation_future = Arc::clone(&self.shared.operations).lock_owned();
        let operation = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                operation = operation_future => operation,
            }
        } else {
            operation_future.await
        };
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(AxiomError::Cancelled);
        }
        let store = Arc::clone(&self.store);
        let shared = Arc::clone(&self.shared);
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            store.delete().map_err(AxiomError::Storage)?;

            // Deletion is the commit point. If the OS store rejects it, retain
            // memory so state cannot disagree with the next launch. Keeping
            // both halves in this task also makes caller cancellation safe.
            let mut state = shared
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.credential = None;
            state.unavailable_detail = None;
            state.generation = state.generation.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
            Ok(state.revision)
        })
        .await
        .map_err(|error| AxiomError::Storage(format!("credential deletion task failed: {error}")))?
    }

    pub async fn validate(&self) -> ValidationStatus {
        self.validate_with_revision().await.status
    }

    pub async fn validate_with_revision(&self) -> VersionedValidationStatus {
        let (snapshot, revision) = self.begin_validation();
        let Some(snapshot) = snapshot else {
            let status = self
                .unavailable_detail()
                .map_or(ValidationStatus::Missing, ValidationStatus::Unavailable);
            return VersionedValidationStatus { status, revision };
        };
        let response = self
            .client
            .get(self.endpoint("/api/v1/auth/api-key"))
            .bearer_auth(&snapshot.credential)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if let Some(status) = self.changed_validation_status(&snapshot) {
                    return VersionedValidationStatus { status, revision };
                }
                return VersionedValidationStatus {
                    status: ValidationStatus::Unavailable(format!(
                        "could not reach Axiom to validate this login: {error}"
                    )),
                    revision,
                };
            }
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            return match self.clear_if_current_async(&snapshot).await {
                Ok(Some(clear_revision)) => VersionedValidationStatus {
                    status: ValidationStatus::Invalid,
                    revision: clear_revision,
                },
                Ok(None) => VersionedValidationStatus {
                    status: self
                        .changed_validation_status(&snapshot)
                        .unwrap_or(ValidationStatus::Invalid),
                    revision,
                },
                Err(error) => {
                    tracing::warn!(%error, "could not clear rejected Axiom credential");
                    VersionedValidationStatus {
                        status: self
                            .changed_validation_status(&snapshot)
                            .unwrap_or(ValidationStatus::Invalid),
                        revision,
                    }
                }
            };
        }
        if !response.status().is_success() {
            if let Some(status) = self.changed_validation_status(&snapshot) {
                return VersionedValidationStatus { status, revision };
            }
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(format!(
                    "Axiom credential validation returned HTTP {}",
                    response.status()
                )),
                revision,
            };
        }
        let status = match response.json::<ApiKeyStatusResponse>().await {
            Ok(value) => {
                self.changed_validation_status(&snapshot)
                    .unwrap_or(ValidationStatus::Valid(AccountStatus {
                        key_id: value.id,
                        key_name: value.name,
                        source: snapshot.source,
                        generation: snapshot.generation,
                        revision,
                    }))
            }
            Err(_) => self
                .changed_validation_status(&snapshot)
                .unwrap_or_else(|| {
                    ValidationStatus::Unavailable(
                        "Axiom returned an invalid credential-validation response".into(),
                    )
                }),
        };
        VersionedValidationStatus { status, revision }
    }

    pub async fn start_browser_login(&self) -> Result<BrowserLogin> {
        if let Some(detail) = self.unavailable_detail() {
            return Err(AxiomError::Storage(detail));
        }
        let auth_generation = self.generation();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let verifier = random_urlsafe(32)?;
        let state = random_urlsafe(32)?;
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let callback_uri = format!("http://127.0.0.1:{port}/callback");
        let client_id_path = self.store.client_id_path().to_owned();
        let client_id =
            tokio::task::spawn_blocking(move || load_or_create_client_id(client_id_path.as_path()))
                .await
                .map_err(|error| {
                    AxiomError::Storage(format!("client identity task failed: {error}"))
                })??;
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
            auth_generation,
        })
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

    async fn finish_exchange(
        &self,
        response: ExchangeResponse,
        expected_generation: u64,
        cancellation: Option<&CancellationToken>,
    ) -> Result<AccountStatus> {
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
        let version = self
            .save_credential_async(&token, Some(expected_generation), cancellation)
            .await?;
        Ok(AccountStatus {
            key_id: key.id,
            key_name: key.name,
            source: self.credential_source(),
            generation: version.generation,
            revision: version.revision,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        self.api_base_url
            .join(path)
            .expect("static API path is valid")
    }

    fn generation(&self) -> u64 {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn begin_validation(&self) -> (Option<CredentialSnapshot>, u64) {
        let mut state = self
            .shared
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        let snapshot = state
            .credential
            .as_ref()
            .map(|credential| CredentialSnapshot {
                credential: credential.expose_secret().to_owned(),
                source: state.source,
                generation: state.generation,
            });
        (snapshot, revision)
    }

    fn unavailable_detail(&self) -> Option<String> {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unavailable_detail
            .clone()
    }

    fn changed_validation_status(&self, snapshot: &CredentialSnapshot) -> Option<ValidationStatus> {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unchanged = state.generation == snapshot.generation
            && state
                .credential
                .as_ref()
                .is_some_and(|credential| credential.expose_secret() == snapshot.credential);
        if unchanged {
            None
        } else if state.credential.is_none() {
            Some(
                state
                    .unavailable_detail
                    .clone()
                    .map_or(ValidationStatus::Missing, ValidationStatus::Unavailable),
            )
        } else {
            Some(ValidationStatus::Unavailable(
                "Axiom authorization changed while validation was in progress; retry validation"
                    .into(),
            ))
        }
    }

    async fn clear_if_current_async(&self, snapshot: &CredentialSnapshot) -> Result<Option<u64>> {
        let operation = Arc::clone(&self.shared.operations).lock_owned().await;
        let store = Arc::clone(&self.store);
        let shared = Arc::clone(&self.shared);
        let checked_credential = snapshot.credential.clone();
        let checked_generation = snapshot.generation;
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            let is_current = {
                let state = shared
                    .state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.generation == checked_generation
                    && state
                        .credential
                        .as_ref()
                        .is_some_and(|credential| credential.expose_secret() == checked_credential)
            };
            if !is_current {
                return Ok(None);
            }

            store.delete().map_err(AxiomError::Storage)?;
            let mut state = shared
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The owned operation guard spans both persistence and state commit,
            // even if the caller drops its future while deletion is blocked.
            state.credential = None;
            state.unavailable_detail = None;
            state.generation = state.generation.wrapping_add(1);
            state.revision = state.revision.wrapping_add(1);
            Ok(Some(state.revision))
        })
        .await
        .map_err(|error| AxiomError::Storage(format!("credential deletion task failed: {error}")))?
    }
}

#[derive(Debug)]
struct EnvironmentStore {
    client_id_path: PathBuf,
}

#[derive(Debug)]
struct UnavailableStore {
    detail: String,
    client_id_path: PathBuf,
}

impl CredentialStore for UnavailableStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::SystemKeyring
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        Err(self.detail.clone())
    }

    fn save(&self, _credential: &str) -> std::result::Result<(), String> {
        Err(self.detail.clone())
    }

    fn delete(&self) -> std::result::Result<(), String> {
        Err(self.detail.clone())
    }

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
    }
}

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

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
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
    auth_generation: u64,
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
            .field("auth_generation", &self.auth_generation)
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
                    if let Some(account) = self.poll_exchange(&cancellation).await? {
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
                        && let Some(account) = self.poll_exchange(&cancellation).await?
                    {
                        return Ok(account);
                    }
                }
            }
        }
    }

    async fn poll_exchange(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Option<AccountStatus>> {
        let exchange = self.manager.exchange(ExchangeRequest {
            authorization_id: &self.authorization_id,
            device_code: self.device_code.expose_secret(),
            code_verifier: self.verifier.expose_secret(),
        });
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            response = exchange => response?,
        };
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        if response.status == "pending" {
            return Ok(None);
        }
        // Cancellation is honored through mutation-lane acquisition. Once
        // persistence starts, await it to its durable commit point so the
        // caller always receives the account state that was actually saved.
        self.manager
            .finish_exchange(response, self.auth_generation, Some(cancellation))
            .await
            .map(Some)
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

    let lock_path = directory.join(".client-id.lock");
    let mut lock_options = OpenOptions::new();
    lock_options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        lock_options.mode(0o600);
    }
    let lock = lock_options.open(&lock_path)?;
    lock.lock_exclusive().map_err(|error| {
        AxiomError::Storage(format!(
            "could not coordinate installation identity: {error}"
        ))
    })?;
    match read_client_id(path) {
        Ok(client_id) => return Ok(client_id),
        Err(AxiomError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let client_id = random_urlsafe(32)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(client_id.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        set_file_permissions(&temporary)?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(client_id)
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
            "invalid Axiom API key; expected a key beginning with axm_".into(),
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
    device_code: &'a str,
    code_verifier: &'a str,
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
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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

    struct BlockingStore {
        credential: Mutex<Option<String>>,
        save_started: tokio::sync::Notify,
        save_gate: (Mutex<bool>, Condvar),
        delete_started: AtomicBool,
        client_id_path: PathBuf,
    }

    impl BlockingStore {
        fn release_save(&self) {
            *self.save_gate.0.lock().expect("save gate lock") = true;
            self.save_gate.1.notify_all();
        }
    }

    impl CredentialStore for BlockingStore {
        fn source(&self) -> CredentialSource {
            CredentialSource::ProtectedFile
        }

        fn load(&self) -> std::result::Result<Option<String>, String> {
            Ok(self.credential.lock().expect("credential lock").clone())
        }

        fn save(&self, credential: &str) -> std::result::Result<(), String> {
            self.save_started.notify_one();
            let released = self.save_gate.0.lock().expect("save gate lock");
            drop(
                self.save_gate
                    .1
                    .wait_while(released, |released| !*released)
                    .expect("save gate wait"),
            );
            *self.credential.lock().expect("credential lock") = Some(credential.to_owned());
            Ok(())
        }

        fn delete(&self) -> std::result::Result<(), String> {
            self.delete_started.store(true, Ordering::Release);
            *self.credential.lock().expect("credential lock") = None;
            Ok(())
        }

        fn client_id_path(&self) -> &Path {
            &self.client_id_path
        }
    }

    fn manager_with_store(store: Arc<dyn CredentialStore>, api_base_url: &str) -> AuthManager {
        let credential = store
            .load()
            .expect("load test credential")
            .map(SecretString::from);
        AuthManager {
            shared: Arc::new(AuthShared {
                state: RwLock::new(AuthState {
                    credential,
                    source: store.source(),
                    generation: 0,
                    revision: 0,
                    unavailable_detail: None,
                }),
                operations: Arc::new(AsyncMutex::new(())),
            }),
            store,
            client: Client::builder().build().expect("auth client"),
            api_base_url: Url::parse(api_base_url).expect("API URL"),
            web_base_url: Url::parse("https://axiom.stream").expect("web URL"),
            launch_browser: false,
        }
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

        fn client_id_path(&self) -> &Path {
            &self.client_id_path
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
        let verifier = body["code_verifier"]
            .as_str()
            .expect("exchange must carry the PKCE verifier");
        let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(
            state.challenge.lock().expect("challenge lock").as_deref(),
            Some(actual.as_str())
        );
        assert_eq!(body.as_object().map(serde_json::Map::len), Some(3));
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

    #[derive(Default)]
    struct DelayedValidationState {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    async fn delayed_valid_authorization(
        State(state): State<Arc<DelayedValidationState>>,
    ) -> Json<Value> {
        state.entered.notify_one();
        state.release.notified().await;
        Json(json!({ "id": "old-key-id", "name": "Old key" }))
    }

    #[derive(Default)]
    struct OutOfOrderValidationState {
        requests: AtomicUsize,
        first_entered: tokio::sync::Notify,
        release_first: tokio::sync::Notify,
    }

    async fn out_of_order_authorization(
        State(state): State<Arc<OutOfOrderValidationState>>,
    ) -> (AxumStatus, Json<Value>) {
        if state.requests.fetch_add(1, Ordering::AcqRel) == 0 {
            state.first_entered.notify_one();
            state.release_first.notified().await;
            return (
                AxumStatus::SERVICE_UNAVAILABLE,
                Json(json!({ "detail": "stale outage" })),
            );
        }
        (
            AxumStatus::OK,
            Json(json!({ "id": "current-key-id", "name": "Current key" })),
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
        let client_id_path = state.identity_directory.path().join("client-id");
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            credential: Mutex::new(None),
            client_id_path,
        });
        let manager = AuthManager {
            shared: Arc::new(AuthShared {
                state: RwLock::new(AuthState {
                    credential: None,
                    source: store.source(),
                    generation: 0,
                    revision: 0,
                    unavailable_detail: None,
                }),
                operations: Arc::new(AsyncMutex::new(())),
            }),
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
        let path = directory.path().join("client-id");
        let first = load_or_create_client_id(&path).expect("create client ID");
        let second = load_or_create_client_id(&path).expect("reload client ID");

        assert_eq!(first, second);
        assert_eq!(first.len(), 43);
        let name = client_name(&first);
        assert!(name.ends_with(&first[first.len() - 6..]));
        assert!(name.chars().count() <= 80);
    }

    #[tokio::test]
    async fn unavailable_platform_credentials_start_signed_out_instead_of_failing() {
        let directory = tempfile::tempdir().expect("auth paths");
        let paths = crate::paths::AxiomPaths::from_roots(
            directory.path().join("config"),
            directory.path().join("data"),
        );
        let manager = AuthManager::new_unavailable(
            "https://api.axiom.stream",
            Duration::from_secs(1),
            &paths,
            "credential manager unavailable",
        )
        .expect("construct signed-out manager");

        assert!(!manager.has_credential());
        let status = manager.validate_with_revision().await;
        assert!(
            matches!(status.status, ValidationStatus::Unavailable(detail) if detail.contains("credential manager unavailable"))
        );
        assert!(matches!(
            manager.start_browser_login().await,
            Err(AxiomError::Storage(detail)) if detail.contains("credential manager unavailable")
        ));
        assert!(matches!(
            manager
                .save_pasted_key_async("axm_1234567890123456")
                .await,
            Err(AxiomError::Storage(detail)) if detail.contains("credential manager unavailable")
        ));
    }

    #[test]
    fn browser_login_debug_output_redacts_exchange_secrets() {
        let text = format!(
            "{:?}",
            AuthState {
                credential: Some(SecretString::from("axm_super-secret".to_owned())),
                source: CredentialSource::ProtectedFile,
                generation: 0,
                revision: 0,
                unavailable_detail: None,
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
    async fn rejected_persisted_credentials_are_cleared() {
        let (manager, _) = mock_manager().await;
        manager
            .save_pasted_key_async("axm_1234567890123456")
            .await
            .expect("save synthetic key");

        assert_eq!(manager.validate().await, ValidationStatus::Invalid);
        assert!(!manager.has_credential());
        assert!(manager.store.load().expect("load credential").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn credential_store_mutations_are_offloaded_and_serialized() {
        let directory = tempfile::tempdir().expect("identity temp directory");
        let store = Arc::new(BlockingStore {
            credential: Mutex::new(None),
            save_started: tokio::sync::Notify::new(),
            save_gate: (Mutex::new(false), Condvar::new()),
            delete_started: AtomicBool::new(false),
            client_id_path: directory.path().join("client-id"),
        });
        let manager = manager_with_store(Arc::<BlockingStore>::clone(&store), "http://127.0.0.1:1");

        let saving_manager = manager.clone();
        let saving = tokio::spawn(async move {
            saving_manager
                .save_pasted_key_async("axm_1234567890123456")
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), store.save_started.notified())
            .await
            .expect("blocking save never started");

        // This is a single-thread Tokio runtime. Reaching this point while the
        // store is blocked proves the OS operation left the cooperative lane.
        let logout_manager = manager.clone();
        let logout = tokio::spawn(async move { logout_manager.logout_async().await });
        tokio::task::yield_now().await;
        assert!(!store.delete_started.load(Ordering::Acquire));

        store.release_save();
        saving.await.expect("save task").expect("save credential");
        logout
            .await
            .expect("logout task")
            .expect("delete credential");
        assert!(store.delete_started.load(Ordering::Acquire));
        assert!(!manager.has_credential());
        assert!(store.load().expect("load credential").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_credential_mutations_honor_cancellation_before_commit() {
        let directory = tempfile::tempdir().expect("identity temp directory");
        let store = Arc::new(BlockingStore {
            credential: Mutex::new(None),
            save_started: tokio::sync::Notify::new(),
            save_gate: (Mutex::new(false), Condvar::new()),
            delete_started: AtomicBool::new(false),
            client_id_path: directory.path().join("client-id"),
        });
        let manager = manager_with_store(Arc::<BlockingStore>::clone(&store), "http://127.0.0.1:1");

        let first_manager = manager.clone();
        let first = tokio::spawn(async move {
            first_manager
                .save_pasted_key_async("axm_1234567890123456")
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), store.save_started.notified())
            .await
            .expect("blocking save never started");

        let logout_cancellation = CancellationToken::new();
        let queued_logout_manager = manager.clone();
        let queued_logout_token = logout_cancellation.clone();
        let queued_logout = tokio::spawn(async move {
            queued_logout_manager
                .logout_with_cancellation(&queued_logout_token)
                .await
        });
        let save_cancellation = CancellationToken::new();
        let queued_save_manager = manager.clone();
        let queued_save_token = save_cancellation.clone();
        let queued_save = tokio::spawn(async move {
            queued_save_manager
                .save_pasted_key_with_cancellation("axm_abcdefghijklmnop", &queued_save_token)
                .await
        });
        tokio::task::yield_now().await;
        logout_cancellation.cancel();
        save_cancellation.cancel();

        assert!(matches!(
            queued_logout.await.expect("logout task"),
            Err(AxiomError::Cancelled)
        ));
        assert!(matches!(
            queued_save.await.expect("save task"),
            Err(AxiomError::Cancelled)
        ));
        assert!(!store.delete_started.load(Ordering::Acquire));

        store.release_save();
        first.await.expect("first save task").expect("first save");
        assert_eq!(
            manager.credential().expect("committed credential"),
            "axm_1234567890123456"
        );
    }

    #[tokio::test]
    async fn later_validation_revision_wins_when_an_older_request_finishes_last() {
        let delayed = Arc::new(OutOfOrderValidationState::default());
        let app = Router::new()
            .route("/api/v1/auth/api-key", get(out_of_order_authorization))
            .with_state(delayed.clone());
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

        let directory = tempfile::tempdir().expect("identity temp directory");
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            credential: Mutex::new(Some("axm_1234567890123456".into())),
            client_id_path: directory.path().join("client-id"),
        });
        let manager = manager_with_store(store, &base);
        let old_manager = manager.clone();
        let older = tokio::spawn(async move { old_manager.validate_with_revision().await });
        tokio::time::timeout(Duration::from_secs(1), delayed.first_entered.notified())
            .await
            .expect("first validation did not start");

        let newer = manager.validate_with_revision().await;
        delayed.release_first.notify_one();
        let older = older.await.expect("older validation task");

        assert!(matches!(newer.status, ValidationStatus::Valid(_)));
        assert!(matches!(older.status, ValidationStatus::Unavailable(_)));
        assert!(newer.revision > older.revision);
    }

    #[tokio::test]
    async fn logout_invalidates_an_in_flight_successful_validation() {
        let delayed = Arc::new(DelayedValidationState::default());
        let app = Router::new()
            .route("/api/v1/auth/api-key", get(delayed_valid_authorization))
            .with_state(delayed.clone());
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

        let directory = tempfile::tempdir().expect("identity temp directory");
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore {
            credential: Mutex::new(Some("axm_1234567890123456".into())),
            client_id_path: directory.path().join("client-id"),
        });
        let manager = manager_with_store(store, &base);
        let validating_manager = manager.clone();
        let validation =
            tokio::spawn(async move { validating_manager.validate_with_revision().await });
        tokio::time::timeout(Duration::from_secs(1), delayed.entered.notified())
            .await
            .expect("validation request never arrived");

        let logout_revision = manager
            .logout_with_cancellation(&CancellationToken::new())
            .await
            .expect("logout");
        delayed.release.notify_one();

        let validation = validation.await.expect("validation task");
        assert_eq!(validation.status, ValidationStatus::Missing);
        assert!(validation.revision < logout_revision);
    }

    #[tokio::test]
    async fn logout_prevents_pending_browser_login_from_restoring_a_credential() {
        let (manager, _) = mock_manager().await;
        let expected_generation = manager.generation();
        manager.logout_async().await.expect("logout");

        let result = manager
            .finish_exchange(
                ExchangeResponse {
                    status: "complete".into(),
                    token: Some("axm_mock-browser-exchange-token".into()),
                    key: Some(ApiKeyResponse {
                        id: "key-id".into(),
                        name: "AxiomCLI".into(),
                    }),
                },
                expected_generation,
                None,
            )
            .await;

        assert!(matches!(result, Err(AxiomError::InvalidTransition(_))));
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
