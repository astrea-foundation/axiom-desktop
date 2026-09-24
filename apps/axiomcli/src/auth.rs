//! Native account sessions and OS credential-store integration.
//!
//! Native login is approved in the system browser and bound to this process
//! with a loopback callback, a one-time device secret, and PKCE S256. The
//! durable credential is a rotating refresh token kept only in the platform
//! credential store. Access tokens are short-lived and memory-only. A manual
//! `AXIOM_API_KEY` is an explicit automation credential and never enters the
//! native-session persistence path.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use async_trait::async_trait;
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
const KEYRING_ACCOUNT_PREFIX: &str = "native-refresh-token-v1";
const MAX_TOKEN_BYTES: usize = 8 * 1024;
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;
const CREDENTIAL_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(3);
const CREDENTIAL_OPERATION_TIMEOUT: Duration = Duration::from_secs(3);
const CREDENTIAL_TIMEOUT_MARKER: &str =
    "timed out; the system credential store remains unavailable";
const DEFAULT_MINIMUM_ACCESS_LIFETIME: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Environment,
    SystemKeyring,
}

impl CredentialSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Environment => "AXIOM_API_KEY",
            Self::SystemKeyring => "system credential store",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethod {
    Passkey,
    Google,
    Password,
    #[serde(rename = "ethereum")]
    EthereumWallet,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccountProfile {
    pub id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub verified_email: Option<String>,
    pub linked_methods: Vec<LoginMethod>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSessionStatus {
    pub id: Option<String>,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountStatus {
    pub account: AccountProfile,
    pub session: NativeSessionStatus,
    pub source: CredentialSource,
    generation: u64,
    revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationStatus {
    Missing,
    Valid(AccountStatus),
    Expired,
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedValidationStatus {
    pub status: ValidationStatus,
    pub revision: u64,
}

/// A bearer credential leased before a secure operation begins. Callers must
/// not log or serialize it. Obtaining a lease may rotate the native session;
/// once provider ciphertext may have been dispatched, callers must never retry
/// the inference merely because authorization subsequently fails.
pub struct AccessTokenLease {
    token: SecretString,
    pub expires_at: Option<DateTime<Utc>>,
}

pub(crate) struct AccountAccessTokenLease {
    access: AccessTokenLease,
    account_id: String,
    generation: u64,
}

impl AccountAccessTokenLease {
    pub(crate) fn expose_for_authorization(&self) -> &str {
        self.access.expose_for_authorization()
    }
}

impl std::fmt::Debug for AccountAccessTokenLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountAccessTokenLease")
            .field("access", &"[REDACTED]")
            .field("account_id", &self.account_id)
            .field("generation", &self.generation)
            .finish()
    }
}

impl std::fmt::Debug for AccessTokenLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessTokenLease")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl AccessTokenLease {
    #[must_use]
    pub fn expose_for_authorization(&self) -> &str {
        self.token.expose_secret()
    }
}

#[async_trait]
pub trait AccessTokenSource: Send + Sync {
    async fn access_token(
        &self,
        minimum_remaining: Duration,
        cancellation: &CancellationToken,
    ) -> Result<AccessTokenLease>;
}

trait CredentialStore: Send + Sync {
    fn source(&self) -> CredentialSource;
    fn load(&self) -> std::result::Result<Option<String>, String>;
    fn save(&self, credential: &str) -> std::result::Result<(), String>;
    fn delete(&self) -> std::result::Result<(), String>;
    fn client_id_path(&self) -> &Path;
    fn refresh_lock_path(&self) -> &Path;
}

#[derive(Debug)]
struct KeyringStore {
    client_id_path: PathBuf,
    refresh_lock_path: PathBuf,
    account_name: String,
}

impl KeyringStore {
    fn entry(&self) -> std::result::Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account_name)
            .map_err(|error| format!("system credential store is unavailable: {error}"))
    }
}

impl CredentialStore for KeyringStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::SystemKeyring
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "could not read the system credential store: {error}"
            )),
        }
    }

    fn save(&self, credential: &str) -> std::result::Result<(), String> {
        self.entry()?
            .set_password(credential)
            .map_err(|error| format!("could not write the system credential store: {error}"))
    }

    fn delete(&self) -> std::result::Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!(
                "could not clear the system credential store: {error}"
            )),
        }
    }

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
    }

    fn refresh_lock_path(&self) -> &Path {
        &self.refresh_lock_path
    }
}

#[derive(Debug)]
struct EnvironmentStore {
    client_id_path: PathBuf,
    refresh_lock_path: PathBuf,
}

impl CredentialStore for EnvironmentStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::Environment
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }

    fn save(&self, _credential: &str) -> std::result::Result<(), String> {
        Err("AXIOM_API_KEY is an environment-owned automation credential".into())
    }

    fn delete(&self) -> std::result::Result<(), String> {
        Err("unset AXIOM_API_KEY to remove the automation credential".into())
    }

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
    }

    fn refresh_lock_path(&self) -> &Path {
        &self.refresh_lock_path
    }
}

#[derive(Debug)]
struct UnavailableStore {
    detail: String,
    client_id_path: PathBuf,
    refresh_lock_path: PathBuf,
}

/// Process-local credential storage used only by debug test runners. This
/// keeps PTY/process tests independent of the host keyring without ever
/// introducing a plaintext refresh-token file format.
#[cfg(debug_assertions)]
#[derive(Debug)]
struct EphemeralTestStore {
    credential: RwLock<Option<String>>,
    client_id_path: PathBuf,
    refresh_lock_path: PathBuf,
}

#[cfg(debug_assertions)]
impl CredentialStore for EphemeralTestStore {
    fn source(&self) -> CredentialSource {
        CredentialSource::SystemKeyring
    }

    fn load(&self) -> std::result::Result<Option<String>, String> {
        Ok(self
            .credential
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    fn save(&self, credential: &str) -> std::result::Result<(), String> {
        *self
            .credential
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(credential.to_owned());
        Ok(())
    }

    fn delete(&self) -> std::result::Result<(), String> {
        *self
            .credential
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    fn client_id_path(&self) -> &Path {
        &self.client_id_path
    }

    fn refresh_lock_path(&self) -> &Path {
        &self.refresh_lock_path
    }
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

    fn refresh_lock_path(&self) -> &Path {
        &self.refresh_lock_path
    }
}

#[derive(Clone)]
struct CachedAccessToken {
    token: SecretString,
    expires_at: DateTime<Utc>,
}

struct AuthState {
    automation_key: Option<SecretString>,
    refresh_token: Option<SecretString>,
    access_token: Option<CachedAccessToken>,
    account: Option<AccountProfile>,
    session: Option<NativeSessionStatus>,
    source: CredentialSource,
    generation: u64,
    revision: u64,
    unavailable_detail: Option<String>,
    external_change_pending: bool,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthState")
            .field(
                "automation_key",
                &self.automation_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("account", &self.account)
            .field("session", &self.session)
            .field("source", &self.source)
            .field("generation", &self.generation)
            .field("revision", &self.revision)
            .field("unavailable_detail", &self.unavailable_detail)
            .field("external_change_pending", &self.external_change_pending)
            .finish()
    }
}

struct AuthShared {
    state: RwLock<AuthState>,
    operations: Arc<AsyncMutex<()>>,
}

#[derive(Clone)]
pub struct AuthManager {
    shared: Arc<AuthShared>,
    store: Arc<dyn CredentialStore>,
    client: Client,
    api_base_url: Url,
    auth_base_url: Url,
    launch_browser: bool,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthManager")
            .field("state", &self.shared.state)
            .field("api_base_url", &self.api_base_url)
            .field("auth_base_url", &self.auth_base_url)
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
                "the system credential store did not respond; Axiom is running signed out",
            ),
        }
    }

    /// Build a signed-out, process-local auth surface for the deterministic
    /// debug runners. Production builds cannot call this constructor.
    #[cfg(debug_assertions)]
    pub fn new_ephemeral_test(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
    ) -> Result<Self> {
        let (client, api_base_url, auth_base_url) = build_client(api_base_url, timeout)?;
        let (_, refresh_lock_path) = credential_scope(&api_base_url, &paths.identity_path());
        let store: Arc<dyn CredentialStore> = Arc::new(EphemeralTestStore {
            credential: RwLock::new(None),
            client_id_path: paths.identity_path(),
            refresh_lock_path,
        });
        let source = store.source();
        let mut manager = Self::from_parts(
            store,
            client,
            api_base_url,
            auth_base_url,
            AuthState {
                automation_key: None,
                refresh_token: None,
                access_token: None,
                account: None,
                session: None,
                source,
                generation: 0,
                revision: 0,
                unavailable_detail: None,
                external_change_pending: false,
            },
        );
        manager.launch_browser = false;
        Ok(manager)
    }

    pub fn new_with_paths(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
    ) -> Result<Self> {
        let (client, api_base_url, auth_base_url) = build_client(api_base_url, timeout)?;
        let (keyring_account, refresh_lock_path) =
            credential_scope(&api_base_url, &paths.identity_path());
        if let Ok(value) = std::env::var("AXIOM_API_KEY") {
            validate_api_key_shape(&value)?;
            let store: Arc<dyn CredentialStore> = Arc::new(EnvironmentStore {
                client_id_path: paths.identity_path(),
                refresh_lock_path,
            });
            let source = store.source();
            return Ok(Self::from_parts(
                store,
                client,
                api_base_url,
                auth_base_url,
                AuthState {
                    automation_key: Some(SecretString::from(value)),
                    refresh_token: None,
                    access_token: None,
                    account: None,
                    session: None,
                    source,
                    generation: 0,
                    revision: 0,
                    unavailable_detail: None,
                    external_change_pending: false,
                },
            ));
        }

        if let Ok(requested) = std::env::var("AXIOMCLI_CREDENTIAL_STORE")
            && requested != "keyring"
        {
            return Err(AxiomError::Config(
                "native sessions require the system credential store; plaintext file storage is not supported"
                    .into(),
            ));
        }
        let keyring: Arc<dyn CredentialStore> = Arc::new(KeyringStore {
            client_id_path: paths.identity_path(),
            refresh_lock_path,
            account_name: keyring_account,
        });
        let _credential_lock = acquire_refresh_file_lock(keyring.refresh_lock_path())?;
        match keyring.load() {
            Ok(token) => {
                let refresh_token = if let Some(value) = token {
                    if validate_token_shape(&value, "stored refresh token").is_ok() {
                        Some(SecretString::from(value))
                    } else {
                        if let Err(error) = keyring.delete() {
                            return Self::new_unavailable(
                                api_base_url.as_str(),
                                timeout,
                                paths,
                                &format!(
                                    "the system credential store contains an invalid native session and could not be cleared: {error}"
                                ),
                            );
                        }
                        None
                    }
                } else {
                    None
                };
                let source = keyring.source();
                Ok(Self::from_parts(
                    keyring,
                    client,
                    api_base_url,
                    auth_base_url,
                    AuthState {
                        automation_key: None,
                        refresh_token,
                        access_token: None,
                        account: None,
                        session: None,
                        source,
                        generation: 0,
                        revision: 0,
                        unavailable_detail: None,
                        external_change_pending: false,
                    },
                ))
            }
            Err(error) => Self::new_unavailable(
                api_base_url.as_str(),
                timeout,
                paths,
                &format!(
                    "the system credential store is unavailable: {error}; sign-in remains disabled until it is restored"
                ),
            ),
        }
    }

    fn new_unavailable(
        api_base_url: &str,
        timeout: Duration,
        paths: &crate::paths::AxiomPaths,
        detail: &str,
    ) -> Result<Self> {
        let (client, api_base_url, auth_base_url) = build_client(api_base_url, timeout)?;
        let (_, refresh_lock_path) = credential_scope(&api_base_url, &paths.identity_path());
        let store: Arc<dyn CredentialStore> = Arc::new(UnavailableStore {
            detail: detail.to_owned(),
            client_id_path: paths.identity_path(),
            refresh_lock_path,
        });
        let source = store.source();
        Ok(Self::from_parts(
            store,
            client,
            api_base_url,
            auth_base_url,
            AuthState {
                automation_key: None,
                refresh_token: None,
                access_token: None,
                account: None,
                session: None,
                source,
                generation: 0,
                revision: 0,
                unavailable_detail: Some(detail.to_owned()),
                external_change_pending: false,
            },
        ))
    }

    fn from_parts(
        store: Arc<dyn CredentialStore>,
        client: Client,
        api_base_url: Url,
        auth_base_url: Url,
        state: AuthState,
    ) -> Self {
        Self {
            shared: Arc::new(AuthShared {
                state: RwLock::new(state),
                operations: Arc::new(AsyncMutex::new(())),
            }),
            store,
            client,
            api_base_url,
            auth_base_url,
            launch_browser: true,
        }
    }

    /// Let a desktop frontend open the validated URL using its own OS session.
    /// Native storage can use isolated XDG directories without changing which
    /// browser handles the authorization link.
    #[must_use]
    pub(crate) fn without_browser_launch(mut self) -> Self {
        self.launch_browser = false;
        self
    }

    #[must_use]
    pub fn has_credential(&self) -> bool {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.automation_key.is_some() || state.refresh_token.is_some()
    }

    #[must_use]
    pub fn credential_source(&self) -> CredentialSource {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .source
    }

    #[must_use]
    pub fn active_account_id(&self) -> Option<String> {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .account
            .as_ref()
            .map(|account| account.id.clone())
    }

    #[must_use]
    pub fn account_status_is_current(&self, status: &AccountStatus) -> bool {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation == status.generation
            && state
                .account
                .as_ref()
                .is_some_and(|account| account.id == status.account.id)
    }

    #[must_use]
    pub const fn account_status_revision(status: &AccountStatus) -> u64 {
        status.revision
    }

    /// Reserve a monotonic publication revision for an in-progress account
    /// transition. The completed validation/logout publication will receive a
    /// later revision and therefore cannot be hidden by the switching reset.
    #[must_use]
    pub fn reserve_account_transition_revision(&self) -> u64 {
        self.next_revision()
    }

    pub async fn access_token_for_secure_operation(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AccessTokenLease> {
        self.access_token(DEFAULT_MINIMUM_ACCESS_LIFETIME, cancellation)
            .await
    }

    /// Lease a native-account bearer for non-inference account APIs. Manual
    /// automation keys are intentionally excluded from Desktop billing.
    pub(crate) async fn access_token_for_account_operation(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AccountAccessTokenLease> {
        if self.credential_source() == CredentialSource::Environment {
            return Err(AxiomError::InvalidTransition(
                "this operation requires a native Axiom account session; AXIOM_API_KEY is automation-only"
                    .into(),
            ));
        }
        let access = self
            .access_token(Duration::from_secs(30), cancellation)
            .await?;
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let account_id = state
            .account
            .as_ref()
            .map(|account| account.id.clone())
            .ok_or_else(|| {
                AxiomError::InvalidTransition(
                    "refresh account status before using account services".into(),
                )
            })?;
        Ok(AccountAccessTokenLease {
            access,
            account_id,
            generation: state.generation,
        })
    }

    pub(crate) fn account_api_origin(&self) -> &Url {
        &self.api_base_url
    }

    pub(crate) fn account_access_is_current(&self, lease: &AccountAccessTokenLease) -> bool {
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation == lease.generation
            && state
                .account
                .as_ref()
                .is_some_and(|account| account.id == lease.account_id)
    }

    pub async fn validate(&self) -> ValidationStatus {
        self.validate_with_revision().await.status
    }

    pub async fn validate_with_revision(&self) -> VersionedValidationStatus {
        let revision = self.next_revision();
        if let Some(detail) = self.unavailable_detail() {
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(detail),
                revision,
            };
        }
        if !self.has_credential() {
            return VersionedValidationStatus {
                status: ValidationStatus::Missing,
                revision,
            };
        }
        let cancellation = CancellationToken::new();
        let lease = match self
            .access_token_internal(Duration::from_secs(30), &cancellation, true)
            .await
        {
            Ok(lease) => lease,
            Err(_) if !self.has_credential() => {
                return VersionedValidationStatus {
                    status: ValidationStatus::Expired,
                    revision: self.current_revision(),
                };
            }
            Err(error) => {
                return VersionedValidationStatus {
                    status: ValidationStatus::Unavailable(error.to_string()),
                    revision,
                };
            }
        };
        let generation = self.generation();
        let response = self
            .client
            .get(self.endpoint("/api/v1/auth/session"))
            .bearer_auth(lease.expose_for_authorization())
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return VersionedValidationStatus {
                    status: ValidationStatus::Unavailable(format!(
                        "could not reach Axiom to validate this session: {error}"
                    )),
                    revision,
                };
            }
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            let clear_revision = self
                .clear_if_generation(generation)
                .await
                .unwrap_or(revision);
            return VersionedValidationStatus {
                status: ValidationStatus::Expired,
                revision: clear_revision,
            };
        }
        if !response.status().is_success() {
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(format!(
                    "Axiom session validation returned HTTP {}",
                    response.status()
                )),
                revision,
            };
        }
        let value = match response.json::<SessionResponse>().await {
            Ok(value) if value.authenticated => value,
            _ => {
                return VersionedValidationStatus {
                    status: ValidationStatus::Unavailable(
                        "Axiom returned an invalid session-validation response".into(),
                    ),
                    revision,
                };
            }
        };
        let Some(account) = value.account else {
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(
                    "Axiom omitted the account from an authenticated session".into(),
                ),
                revision,
            };
        };
        if let Err(error) = validate_account(&account) {
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(error.to_string()),
                revision,
            };
        }
        let session = value.session.map_or_else(
            || NativeSessionStatus {
                id: None,
                expires_at: lease
                    .expires_at
                    .map_or_else(|| Utc::now().to_rfc3339(), |value| value.to_rfc3339()),
            },
            Into::into,
        );
        if let Err(error) = validate_native_session(&session) {
            return VersionedValidationStatus {
                status: ValidationStatus::Unavailable(error.to_string()),
                revision,
            };
        }
        let source = {
            let mut state = self
                .shared
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.generation != generation {
                return VersionedValidationStatus {
                    status: ValidationStatus::Unavailable(
                        "Axiom account changed while validation was in progress; retry validation"
                            .into(),
                    ),
                    revision,
                };
            }
            if state
                .account
                .as_ref()
                .is_some_and(|current| current.id != account.id)
            {
                None
            } else {
                state.account = Some(account.clone());
                state.session = Some(session.clone());
                state.external_change_pending = false;
                Some(state.source)
            }
        };
        let Some(source) = source else {
            // A native session is permanently bound to one account. Treat an
            // identity change from the authoritative session endpoint as a
            // terminal session failure, not as an account switch that could
            // relabel the already-open local store.
            let clear_revision = self
                .clear_if_generation(generation)
                .await
                .unwrap_or(revision);
            return VersionedValidationStatus {
                status: ValidationStatus::Expired,
                revision: clear_revision,
            };
        };
        VersionedValidationStatus {
            status: ValidationStatus::Valid(AccountStatus {
                account,
                session,
                source,
                generation,
                revision,
            }),
            revision,
        }
    }

    pub async fn start_native_login(
        &self,
        method_hint: Option<LoginMethod>,
    ) -> Result<NativeLogin> {
        if self.credential_source() == CredentialSource::Environment {
            return Err(AxiomError::InvalidTransition(
                "AXIOM_API_KEY is active; unset it before starting an account session".into(),
            ));
        }
        if let Some(detail) = self.unavailable_detail() {
            return Err(AxiomError::Storage(detail));
        }
        {
            let credential_lock = self.acquire_credential_lock().await?;
            let persisted = match self.load_persisted_credential(&credential_lock).await {
                Ok(persisted) => persisted,
                Err(error) => {
                    self.clear_local_authorization(Some(error.to_string()));
                    return Err(error);
                }
            };
            if persisted != self.refresh_token_value() {
                self.adopt_external_credential(persisted)?;
            }
        }
        if self.has_credential() {
            return Err(AxiomError::InvalidTransition(
                "sign out of the current Axiom account before starting another native login".into(),
            ));
        }
        let auth_generation = self.generation();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let verifier = random_urlsafe(32)?;
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let callback_uri = format!("http://127.0.0.1:{port}/callback");
        let client_id_path = self.store.client_id_path().to_owned();
        let client_id = bounded_credential_task("client identity", move || {
            load_or_create_client_id(&client_id_path).map_err(|error| error.to_string())
        })
        .await?;
        let response = self
            .client
            .post(self.endpoint("/api/v1/auth/native/authorizations"))
            .json(&CreateAuthorizationRequest {
                code_challenge,
                callback_uri: callback_uri.clone(),
                client_id: client_id.clone(),
                client_name: client_name(&client_id),
                method_hint,
            })
            .send()
            .await
            .map_err(|error| {
                // Keep transport causes (including TLS errors) visible across ACP,
                // without reflecting request URLs into Desktop's error banner.
                let error = anyhow::Error::new(error.without_url());
                AxiomError::Provider(format!("could not start login: {error:#}"))
            })?;
        if !response.status().is_success() {
            return Err(AxiomError::Provider(format!(
                "Axiom rejected the login request with HTTP {}",
                response.status()
            )));
        }
        let created = response
            .json::<CreatedAuthorization>()
            .await
            .map_err(|_| AxiomError::Protocol("invalid native login response from Axiom".into()))?;
        validate_created_authorization(&created, &self.auth_base_url, &callback_uri)?;
        let authorization_url = Url::parse(&created.authorization_url)
            .map_err(|_| AxiomError::Protocol("invalid authorization URL from Axiom".into()))?;
        let browser_opened =
            self.launch_browser && open::that_detached(authorization_url.as_str()).is_ok();
        Ok(NativeLogin {
            manager: self.clone(),
            listener,
            callback_host: format!("127.0.0.1:{port}"),
            authorization_id: created.id,
            device_code: SecretString::from(created.device_code),
            verifier: SecretString::from(verifier),
            state: SecretString::from(created.state),
            user_code: created.user_code,
            authorization_url,
            expires_at: created.expires_at,
            interval: Duration::from_secs(created.interval_seconds.clamp(1, 10)),
            browser_opened,
            auth_generation,
        })
    }

    pub async fn logout_async(&self) -> Result<()> {
        self.logout_with_cancellation(&CancellationToken::new())
            .await
            .map(|_| ())
    }

    pub async fn logout_with_cancellation(&self, cancellation: &CancellationToken) -> Result<u64> {
        if self.credential_source() == CredentialSource::Environment {
            return Err(AxiomError::InvalidTransition(
                "AXIOM_API_KEY is controlled by the process environment; unset it to sign out"
                    .into(),
            ));
        }
        let bearer = match self.access_token(Duration::ZERO, cancellation).await {
            Ok(lease) => Some(lease),
            Err(AxiomError::Cancelled) => return Err(AxiomError::Cancelled),
            Err(_) => None,
        };
        let operation_future = Arc::clone(&self.shared.operations).lock_owned();
        let _operation = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            operation = operation_future => operation,
        };
        if let Some(bearer) = bearer {
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bearer.expose_for_authorization())
                .send()
                .await;
        }
        self.delete_persisted_and_clear().await
    }

    fn endpoint(&self, path: &str) -> Url {
        self.api_base_url
            .join(path)
            .expect("static API path is valid")
    }

    pub(crate) fn generation(&self) -> u64 {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn next_revision(&self) -> u64 {
        let mut state = self
            .shared
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.wrapping_add(1);
        state.revision
    }

    fn current_revision(&self) -> u64 {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    fn unavailable_detail(&self) -> Option<String> {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unavailable_detail
            .clone()
    }

    fn refresh_token_value(&self) -> Option<String> {
        self.shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_token
            .as_ref()
            .map(|value| value.expose_secret().to_owned())
    }

    async fn acquire_credential_lock(&self) -> Result<fs::File> {
        let path = self.store.refresh_lock_path().to_owned();
        let result = bounded_credential_task("native-session coordination", move || {
            acquire_refresh_file_lock(&path).map_err(|error| error.to_string())
        })
        .await;
        if let Err(error) = &result {
            self.clear_local_authorization(Some(error.to_string()));
        }
        result
    }

    async fn load_persisted_credential(
        &self,
        credential_lock: &fs::File,
    ) -> Result<Option<String>> {
        let store = Arc::clone(&self.store);
        let lock = credential_lock.try_clone()?;
        let value = bounded_credential_task("credential read", move || {
            let _lock_keepalive = lock;
            store.load()
        })
        .await?;
        if let Some(value) = &value {
            validate_token_shape(value, "stored refresh token")?;
        }
        Ok(value)
    }

    async fn save_persisted_credential(
        &self,
        credential: String,
        credential_lock: &fs::File,
    ) -> Result<()> {
        let store = Arc::clone(&self.store);
        let cleanup_store = Arc::clone(&self.store);
        let cleanup_credential = credential.clone();
        let lock = credential_lock.try_clone()?;
        let mut task = tokio::task::spawn_blocking(move || (store.save(&credential), lock));
        match tokio::time::timeout(CREDENTIAL_OPERATION_TIMEOUT, &mut task).await {
            Ok(Ok((Ok(()), _lock))) => Ok(()),
            Ok(Ok((Err(error), _lock))) => Err(AxiomError::Storage(error)),
            Ok(Err(_)) => Err(AxiomError::Storage(
                "credential persistence task failed".into(),
            )),
            Err(_) => {
                // `spawn_blocking` cannot cancel an OS keyring call. Preserve
                // the audience lock in the task result, then compare-and-delete
                // the exact late value before releasing that lock. A newly
                // saved credential can therefore never be mistaken for ours.
                tokio::spawn(async move {
                    let Ok((saved, lock)) = task.await else {
                        return;
                    };
                    if saved.is_err() {
                        drop(lock);
                        return;
                    }
                    let _ = tokio::task::spawn_blocking(move || {
                        if cleanup_store.load().ok().flatten().as_deref()
                            == Some(cleanup_credential.as_str())
                        {
                            let _ = cleanup_store.delete();
                        }
                        drop(lock);
                    })
                    .await;
                });
                Err(credential_timeout_error("credential persistence"))
            }
        }
    }

    async fn delete_persisted_credential(
        &self,
        credential_lock: &fs::File,
    ) -> std::result::Result<(), String> {
        let store = Arc::clone(&self.store);
        let lock = credential_lock.try_clone().map_err(|error| {
            format!("could not retain native-session coordination lock: {error}")
        })?;
        let mut task = tokio::task::spawn_blocking(move || (store.delete(), lock));
        match tokio::time::timeout(CREDENTIAL_OPERATION_TIMEOUT, &mut task).await {
            Ok(Ok((result, _lock))) => result,
            Ok(Err(_)) => Err("credential deletion task failed".into()),
            Err(_) => {
                // Keep the audience lock alive until the late deletion has
                // completed. A peer cannot save a replacement that this stale
                // task could subsequently remove.
                tokio::spawn(async move {
                    if let Ok((_result, lock)) = task.await {
                        drop(lock);
                    }
                });
                Err(credential_timeout_error("credential deletion").to_string())
            }
        }
    }

    fn adopt_external_credential(&self, credential: Option<String>) -> Result<()> {
        if let Some(value) = &credential {
            validate_token_shape(value, "stored refresh token")?;
        }
        let mut state = self
            .shared
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.refresh_token = credential.map(SecretString::from);
        state.access_token = None;
        state.account = None;
        state.session = None;
        state.unavailable_detail = None;
        state.external_change_pending = state.refresh_token.is_some();
        state.generation = state.generation.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        Ok(())
    }

    async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<ExchangeResponse> {
        let response = self
            .client
            .post(self.endpoint("/api/v1/auth/native/exchange"))
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
            .map_err(|_| AxiomError::Protocol("invalid native login exchange response".into()))
    }

    async fn finish_exchange(
        &self,
        response: ExchangeResponse,
        expected_generation: u64,
    ) -> Result<AccountStatus> {
        if response.status != "complete" {
            return Err(AxiomError::InvalidTransition(
                "authorization has not been approved yet".into(),
            ));
        }
        let bundle = response
            .bundle()
            .ok_or_else(|| AxiomError::Protocol("completed login omitted session tokens".into()))?;
        let _operation = Arc::clone(&self.shared.operations).lock_owned().await;
        if self.generation() != expected_generation {
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bundle.access_token.expose_secret())
                .send()
                .await;
            return Err(AxiomError::InvalidTransition(
                "Axiom authorization changed while browser sign-in was pending; start sign-in again"
                    .into(),
            ));
        }
        self.commit_bundle(bundle, true).await
    }

    async fn commit_bundle(
        &self,
        bundle: SessionBundle,
        force_identity_generation: bool,
    ) -> Result<AccountStatus> {
        let expected_refresh = self.refresh_token_value();
        let credential_lock = match self.acquire_credential_lock().await {
            Ok(lock) => lock,
            Err(error) => {
                let _ = self
                    .client
                    .delete(self.endpoint("/api/v1/auth/session"))
                    .bearer_auth(bundle.access_token.expose_secret())
                    .send()
                    .await;
                return Err(error);
            }
        };
        self.commit_bundle_under_credential_lock(
            bundle,
            force_identity_generation,
            expected_refresh.as_deref(),
            &credential_lock,
        )
        .await
    }

    async fn commit_bundle_under_credential_lock(
        &self,
        bundle: SessionBundle,
        force_identity_generation: bool,
        expected_refresh: Option<&str>,
        credential_lock: &fs::File,
    ) -> Result<AccountStatus> {
        if let Err(error) = validate_bundle(&bundle) {
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bundle.access_token.expose_secret())
                .send()
                .await;
            let _ = self
                .delete_if_matches_and_clear_under_credential_lock(
                    expected_refresh,
                    credential_lock,
                )
                .await;
            return Err(error);
        }
        let previous_account = self.active_account_id();
        if !force_identity_generation
            && previous_account
                .as_deref()
                .is_some_and(|id| id != bundle.account.id)
        {
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bundle.access_token.expose_secret())
                .send()
                .await;
            let _ = self
                .delete_if_matches_and_clear_under_credential_lock(
                    expected_refresh,
                    credential_lock,
                )
                .await;
            return Err(AxiomError::Protocol(
                "Axiom changed accounts during native-session rotation".into(),
            ));
        }
        let persisted = match self.load_persisted_credential(credential_lock).await {
            Ok(persisted) => persisted,
            Err(error) => {
                // A login/rotation has already issued a new server session,
                // but it is unsafe to overwrite a credential whose current
                // value cannot be compared under the audience lock.
                let _ = self
                    .client
                    .delete(self.endpoint("/api/v1/auth/session"))
                    .bearer_auth(bundle.access_token.expose_secret())
                    .send()
                    .await;
                self.clear_local_authorization(Some(error.to_string()));
                return Err(error);
            }
        };
        if persisted.as_deref() != expected_refresh {
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bundle.access_token.expose_secret())
                .send()
                .await;
            self.adopt_external_credential(persisted)?;
            return Err(account_changed());
        }
        let refresh = bundle.refresh_token.expose_secret().to_owned();
        let persistence = self
            .save_persisted_credential(refresh, credential_lock)
            .await;
        if let Err(error) = persistence {
            // The server has already rotated or issued this session. If the
            // new refresh token cannot be made durable, never retain or reuse
            // the superseded token. Revoke with the newly issued access token
            // where possible, clear the keyring entry, and fail closed.
            let _ = self
                .client
                .delete(self.endpoint("/api/v1/auth/session"))
                .bearer_auth(bundle.access_token.expose_secret())
                .send()
                .await;
            let detail = format!(
                "could not persist the rotated native session in the system credential store: {error}"
            );
            if credential_operation_timed_out(&error) {
                // A blocking platform keyring call cannot be cancelled once
                // the OS has entered it. Its task retains a cloned audience
                // lock until completion, so clear this process immediately
                // but do not race it with a delete that could run first and
                // allow the delayed save to resurrect a revoked token.
                self.clear_local_authorization(Some(detail.clone()));
            } else {
                let _ = self
                    .delete_if_matches_and_clear_under_credential_lock(
                        expected_refresh,
                        credential_lock,
                    )
                    .await;
            }
            return Err(AxiomError::Storage(detail));
        }
        let mut state = self
            .shared
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.refresh_token = Some(bundle.refresh_token);
        state.access_token = Some(CachedAccessToken {
            token: bundle.access_token,
            expires_at: bundle.access_expires_at,
        });
        state.account = Some(bundle.account.clone());
        state.session = Some(bundle.session.clone());
        state.unavailable_detail = None;
        if force_identity_generation {
            state.external_change_pending = false;
        }
        if force_identity_generation
            || previous_account.as_deref() != Some(bundle.account.id.as_str())
        {
            state.generation = state.generation.wrapping_add(1);
        }
        state.revision = state.revision.wrapping_add(1);
        Ok(AccountStatus {
            account: bundle.account,
            session: bundle.session,
            source: state.source,
            generation: state.generation,
            revision: state.revision,
        })
    }

    async fn clear_if_generation(&self, generation: u64) -> Result<u64> {
        let _operation = Arc::clone(&self.shared.operations).lock_owned().await;
        if self.generation() != generation {
            return Ok(self.current_revision());
        }
        self.delete_persisted_and_clear().await
    }

    async fn delete_persisted_and_clear(&self) -> Result<u64> {
        if self.credential_source() == CredentialSource::Environment {
            return Err(AxiomError::InvalidTransition(
                "AXIOM_API_KEY is controlled by the process environment; unset it to sign out"
                    .into(),
            ));
        }
        let expected_refresh = self.refresh_token_value();
        let credential_lock = self.acquire_credential_lock().await?;
        self.delete_if_matches_and_clear_under_credential_lock(
            expected_refresh.as_deref(),
            &credential_lock,
        )
        .await
    }

    async fn delete_if_matches_and_clear_under_credential_lock(
        &self,
        expected_refresh: Option<&str>,
        credential_lock: &fs::File,
    ) -> Result<u64> {
        let deletion = match self.load_persisted_credential(credential_lock).await {
            Ok(persisted)
                if expected_refresh.is_some() && persisted.as_deref() == expected_refresh =>
            {
                self.delete_persisted_credential(credential_lock).await
            }
            Ok(_) => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        let revision = self.clear_local_authorization(deletion.as_ref().err().cloned());
        deletion.map_err(AxiomError::Storage)?;
        Ok(revision)
    }

    fn clear_local_authorization(&self, unavailable_detail: Option<String>) -> u64 {
        let mut state = self
            .shared
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.refresh_token = None;
        state.access_token = None;
        state.account = None;
        state.session = None;
        state.unavailable_detail = unavailable_detail;
        state.external_change_pending = false;
        state.generation = state.generation.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        state.revision
    }

    async fn refresh_under_lock(&self, allow_external_change: bool) -> Result<AccessTokenLease> {
        let credential_lock = self.acquire_credential_lock().await?;
        let persisted = match self.load_persisted_credential(&credential_lock).await {
            Ok(persisted) => persisted,
            Err(error) => {
                self.clear_local_authorization(Some(error.to_string()));
                return Err(error);
            }
        };
        let local_refresh = self.refresh_token_value();
        let changed_externally = persisted != local_refresh;
        let previous_account = self.active_account_id();
        // A peer normally changes the credential by rotating the same account's
        // session. Keep its identity until the server authenticates the new
        // bundle; changing the identity generation here invalidates live work.
        if changed_externally && (previous_account.is_none() || persisted.is_none()) {
            self.adopt_external_credential(persisted.clone())?;
            if !allow_external_change {
                return Err(account_changed());
            }
        }
        let refresh = persisted.ok_or_else(authentication_required)?;
        let response = self
            .client
            .post(self.endpoint("/api/v1/auth/token/refresh"))
            .json(&RefreshRequest {
                refresh_token: &refresh,
            })
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // Once a rotating refresh request has left this process, a
                // lost response is ambiguous: the server may already have
                // consumed the old token. Never retry that token and risk
                // triggering family-reuse revocation.
                let clear_error = self
                    .delete_if_matches_and_clear_under_credential_lock(
                        Some(&refresh),
                        &credential_lock,
                    )
                    .await
                    .err();
                let suffix = clear_error.map_or_else(String::new, |failure| {
                    format!("; the local credential could not be cleared cleanly: {failure}")
                });
                return Err(AxiomError::Provider(format!(
                    "session refresh response was lost; sign in again{suffix}: {error}"
                )));
            }
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            let _ = self
                .delete_if_matches_and_clear_under_credential_lock(Some(&refresh), &credential_lock)
                .await;
            return Err(authentication_required());
        }
        if !response.status().is_success() {
            let status = response.status();
            let clear_error = self
                .delete_if_matches_and_clear_under_credential_lock(Some(&refresh), &credential_lock)
                .await
                .err();
            let suffix = clear_error.map_or_else(String::new, |failure| {
                format!("; the local credential could not be cleared cleanly: {failure}")
            });
            return Err(AxiomError::Provider(format!(
                "Axiom session refresh returned HTTP {status}; sign in again because rotation may have consumed the prior token{suffix}"
            )));
        }
        let Ok(response) = response.json::<RefreshResponse>().await else {
            let _ = self
                .delete_if_matches_and_clear_under_credential_lock(Some(&refresh), &credential_lock)
                .await;
            return Err(AxiomError::Protocol(
                "invalid session refresh response".into(),
            ));
        };
        let Some(bundle) = response.bundle() else {
            let _ = self
                .delete_if_matches_and_clear_under_credential_lock(Some(&refresh), &credential_lock)
                .await;
            return Err(AxiomError::Protocol(
                "session refresh omitted rotated tokens".into(),
            ));
        };
        let switched_account = changed_externally
            && previous_account.is_some()
            && previous_account.as_deref() != Some(bundle.account.id.as_str());
        if switched_account {
            // Preserve the peer's newly rotated credential, but never give it
            // to work registered against the previous account's local store.
            self.adopt_external_credential(Some(refresh.clone()))?;
        }
        self.commit_bundle_under_credential_lock(bundle, false, Some(&refresh), &credential_lock)
            .await?;
        if switched_account && !allow_external_change {
            return Err(account_changed());
        }
        let state = self
            .shared
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let token = state
            .access_token
            .as_ref()
            .expect("committed bundle has access token");
        Ok(AccessTokenLease {
            token: token.token.clone(),
            expires_at: Some(token.expires_at),
        })
    }
}

#[async_trait]
impl AccessTokenSource for AuthManager {
    async fn access_token(
        &self,
        minimum_remaining: Duration,
        cancellation: &CancellationToken,
    ) -> Result<AccessTokenLease> {
        self.access_token_internal(minimum_remaining, cancellation, false)
            .await
    }
}

impl AuthManager {
    async fn access_token_internal(
        &self,
        minimum_remaining: Duration,
        cancellation: &CancellationToken,
        allow_external_change: bool,
    ) -> Result<AccessTokenLease> {
        {
            let state = self
                .shared
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(key) = &state.automation_key {
                return Ok(AccessTokenLease {
                    token: key.clone(),
                    expires_at: None,
                });
            }
            if state.external_change_pending && !allow_external_change {
                return Err(account_changed());
            }
            if let Some(token) = &state.access_token
                && token.expires_at > minimum_expiry(minimum_remaining)
            {
                return Ok(AccessTokenLease {
                    token: token.token.clone(),
                    expires_at: Some(token.expires_at),
                });
            }
        }
        let operation_future = Arc::clone(&self.shared.operations).lock_owned();
        let operation = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            operation = operation_future => operation,
        };
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        {
            let state = self
                .shared
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.external_change_pending && !allow_external_change {
                return Err(account_changed());
            }
            if let Some(token) = &state.access_token
                && token.expires_at > minimum_expiry(minimum_remaining)
            {
                return Ok(AccessTokenLease {
                    token: token.token.clone(),
                    expires_at: Some(token.expires_at),
                });
            }
        }
        // Once this rotating request is dispatched it is intentionally not
        // raced against cancellation. Run it in an owned task as well, so an
        // outer provider/ACP future being dropped cannot lose the only current
        // refresh token. The HTTP client timeout remains the outer bound.
        let manager = self.clone();
        tokio::spawn(async move {
            let _operation = operation;
            manager.refresh_under_lock(allow_external_change).await
        })
        .await
        .map_err(|error| AxiomError::Storage(format!("session refresh task failed: {error}")))?
    }
}

fn minimum_expiry(minimum_remaining: Duration) -> DateTime<Utc> {
    Utc::now()
        + chrono::Duration::from_std(minimum_remaining)
            .unwrap_or_else(|_| chrono::Duration::minutes(2))
}

pub struct NativeLogin {
    manager: AuthManager,
    listener: TcpListener,
    callback_host: String,
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

impl std::fmt::Debug for NativeLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeLogin")
            .field("authorization_id", &self.authorization_id)
            .field("device_code", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field("user_code", &self.user_code)
            // The launch URL embeds the one-time state nonce. Returning it
            // through the explicit login API is required, but Debug output
            // must never turn it into a diagnostic/logging side channel.
            .field("authorization_url", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("browser_opened", &self.browser_opened)
            .finish_non_exhaustive()
    }
}

impl NativeLogin {
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    #[must_use]
    pub fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }

    #[must_use]
    pub fn expires_at(&self) -> &str {
        &self.expires_at
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
                let _ = self.cancel_authorization().await;
                return Err(AxiomError::Provider(
                    "native authorization expired; start sign-in again".into(),
                ));
            }
            tokio::select! {
                () = cancellation.cancelled() => {
                    let _ = self.cancel_authorization().await;
                    return Err(AxiomError::Cancelled);
                },
                _ = ticker.tick() => {
                    if let Some(account) = self.poll_exchange().await? {
                        return Ok(account);
                    }
                }
                accepted = self.listener.accept() => {
                    let (mut socket, _) = accepted?;
                    if handle_loopback_request(
                        &mut socket,
                        self.manager.auth_base_url.origin().ascii_serialization().as_str(),
                        &self.callback_host,
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

    pub async fn cancel(self) -> Result<()> {
        self.cancel_authorization().await
    }

    async fn cancel_authorization(&self) -> Result<()> {
        let response = self
            .manager
            .client
            .delete(self.manager.endpoint(&format!(
                "/api/v1/auth/native/authorizations/{}",
                self.authorization_id
            )))
            .json(&CancelAuthorizationRequest {
                device_code: self.device_code.expose_secret(),
            })
            .send()
            .await
            .map_err(|error| AxiomError::Provider(format!("could not cancel login: {error}")))?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(AxiomError::Provider(format!(
                "Axiom rejected login cancellation with HTTP {}",
                response.status()
            )))
        }
    }

    async fn poll_exchange(&self) -> Result<Option<AccountStatus>> {
        let response = self
            .manager
            .exchange(ExchangeRequest {
                authorization_id: &self.authorization_id,
                device_code: self.device_code.expose_secret(),
                code_verifier: self.verifier.expose_secret(),
            })
            .await?;
        if response.status == "pending" {
            return Ok(None);
        }
        // A complete exchange has issued a refresh-token family. From this
        // point cancellation must not discard the response: persist the
        // rotated refresh token first, then let the caller observe success.
        self.manager
            .finish_exchange(response, self.auth_generation)
            .await
            .map(Some)
    }
}

async fn handle_loopback_request(
    socket: &mut TcpStream,
    allowed_origin: &str,
    expected_host: &str,
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
    let mut origin = None;
    let mut host = None;
    let mut duplicate_security_header = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("origin") {
                duplicate_security_header |= origin.replace(value.trim()).is_some();
            } else if name.eq_ignore_ascii_case("host") {
                duplicate_security_header |= host.replace(value.trim()).is_some();
            }
        }
    }
    if request_line.starts_with("OPTIONS ") {
        let allowed = !duplicate_security_header
            && origin == Some(allowed_origin)
            && host == Some(expected_host);
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
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let parsed = Url::parse(&format!("http://127.0.0.1{target}"));
    let valid = method == "GET"
        && !duplicate_security_header
        && host == Some(expected_host)
        // Top-level browser navigation commonly omits Origin. The unguessable
        // state and exact loopback host/path remain the CSRF binding. If a
        // browser does send Origin, accept only the hosted auth origin.
        && origin.is_none_or(|origin| origin == allowed_origin)
        && parsed.as_ref().is_ok_and(|url| {
            url.path() == "/callback"
                && query_value_occurs_once(url, "authorization_id", authorization_id)
                && query_value_occurs_once(url, "state", expected_state)
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

fn query_value_occurs_once(url: &Url, expected_name: &str, expected_value: &str) -> bool {
    let mut values = url
        .query_pairs()
        .filter(|(name, _)| name == expected_name)
        .map(|(_, value)| value);
    values.next().is_some_and(|value| value == expected_value) && values.next().is_none()
}

async fn write_loopback_response(
    socket: &mut TcpStream,
    status: StatusCode,
    allowed_origin: &str,
    include_cors: bool,
) -> Result<()> {
    let reason = status.canonical_reason().unwrap_or("Response");
    let (title, message, instruction) = if status.is_success() {
        (
            "Back to your space.",
            "Axiom received your approval. Return to the application to finish connecting.",
            "You can close this browser tab.",
        )
    } else {
        (
            "That didn’t connect.",
            "This Axiom callback was rejected.",
            "Return to Axiom and start a new sign-in.",
        )
    };
    // Static brand template; no credential, callback query, or user input is rendered.
    let body = include_str!("../../../packages/brand/templates/desktop-callback.html")
        .replace("{{title}}", title)
        .replace("{{message}}", message)
        .replace("{{instruction}}", instruction);
    let cors = if include_cors {
        format!(
            "Access-Control-Allow-Origin: {allowed_origin}\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Private-Network: true\r\nVary: Origin\r\n"
        )
    } else {
        String::new()
    };
    let response = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n{cors}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        status.as_u16(),
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

fn build_client(api_base_url: &str, timeout: Duration) -> Result<(Client, Url, Url)> {
    let api_base_url = parse_service_url(api_base_url, "API")?;
    let auth_url =
        std::env::var("AXIOM_AUTH_URL").unwrap_or_else(|_| "https://auth.axiom.stream".to_owned());
    let auth_base_url = parse_service_url(&auth_url, "authentication website")?;
    let client = Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| AxiomError::Config(format!("could not build auth client: {error}")))?;
    Ok((client, api_base_url, auth_base_url))
}

fn credential_scope(api_base_url: &Url, client_id_path: &Path) -> (String, PathBuf) {
    let audience = api_base_url.origin().ascii_serialization();
    let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(audience.as_bytes()));
    let keyring_account = format!("{KEYRING_ACCOUNT_PREFIX}-{digest}");
    let lock_directory = client_id_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    (
        keyring_account,
        lock_directory.join(format!(".native-session-{digest}.lock")),
    )
}

async fn bounded_credential_task<T, F>(label: &'static str, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> std::result::Result<T, String> + Send + 'static,
{
    let task = tokio::task::spawn_blocking(operation);
    match tokio::time::timeout(CREDENTIAL_OPERATION_TIMEOUT, task).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(error))) => Err(AxiomError::Storage(error)),
        Ok(Err(_)) => Err(AxiomError::Storage(format!("{label} task failed"))),
        Err(_) => Err(credential_timeout_error(label)),
    }
}

fn credential_timeout_error(label: &str) -> AxiomError {
    AxiomError::Storage(format!("{label} {CREDENTIAL_TIMEOUT_MARKER}"))
}

fn credential_operation_timed_out(error: &AxiomError) -> bool {
    matches!(error, AxiomError::Storage(detail) if detail.contains(CREDENTIAL_TIMEOUT_MARKER))
}

fn acquire_refresh_file_lock(path: &Path) -> Result<fs::File> {
    let parent = path
        .parent()
        .ok_or_else(|| AxiomError::Storage("native-session coordination path is invalid".into()))?;
    fs::create_dir_all(parent)?;
    set_directory_permissions(parent)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    set_file_permissions(path)?;
    file.lock_exclusive().map_err(|error| {
        AxiomError::Storage(format!(
            "could not coordinate native-session refresh: {error}"
        ))
    })?;
    Ok(file)
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
        AxiomError::Storage("local Axiom installation identity path is invalid".into())
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
    let lock = lock_options.open(lock_path)?;
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
        let _ = fs::remove_file(temporary);
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
            "local Axiom installation identity at {} is invalid",
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
    if !value.starts_with("axm_")
        || !(16..=MAX_TOKEN_BYTES).contains(&value.len())
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(AxiomError::Config(
            "invalid AXIOM_API_KEY automation credential".into(),
        ));
    }
    Ok(())
}

fn validate_token_shape(value: &str, label: &str) -> Result<()> {
    if !(32..=MAX_TOKEN_BYTES).contains(&value.len())
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(AxiomError::Protocol(format!(
            "Axiom returned an invalid {label}"
        )));
    }
    Ok(())
}

fn validate_account(account: &AccountProfile) -> Result<()> {
    if account.id.is_empty()
        || account.id.len() > 128
        || !account
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || account.linked_methods.len() > 16
    {
        return Err(AxiomError::Protocol(
            "Axiom returned an invalid account profile".into(),
        ));
    }
    for value in [
        account.display_name.as_deref(),
        account.verified_email.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 320 || value.chars().any(char::is_control) {
            return Err(AxiomError::Protocol(
                "Axiom returned invalid account metadata".into(),
            ));
        }
    }
    if let Some(avatar_url) = &account.avatar_url {
        let url = Url::parse(avatar_url)
            .map_err(|_| AxiomError::Protocol("Axiom returned an invalid avatar URL".into()))?;
        let loopback_http = url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        if avatar_url.len() > 2_048
            || (url.scheme() != "https" && !loopback_http)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(AxiomError::Protocol(
                "Axiom returned an untrusted avatar URL".into(),
            ));
        }
    }
    Ok(())
}

fn validate_native_session(session: &NativeSessionStatus) -> Result<()> {
    if session.id.as_ref().is_some_and(|id| {
        id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    }) {
        return Err(AxiomError::Protocol(
            "Axiom returned an invalid session identifier".into(),
        ));
    }
    let expires_at = DateTime::parse_from_rfc3339(&session.expires_at)
        .map_err(|_| AxiomError::Protocol("Axiom returned invalid session expiry metadata".into()))?
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        return Err(AxiomError::Protocol(
            "Axiom returned an expired native session".into(),
        ));
    }
    Ok(())
}

fn validate_bundle(bundle: &SessionBundle) -> Result<()> {
    validate_token_shape(bundle.access_token.expose_secret(), "access token")?;
    validate_token_shape(bundle.refresh_token.expose_secret(), "refresh token")?;
    validate_account(&bundle.account)?;
    validate_native_session(&bundle.session)?;
    if bundle.access_expires_at <= Utc::now() {
        return Err(AxiomError::Protocol(
            "Axiom returned invalid session expiry metadata".into(),
        ));
    }
    Ok(())
}

fn validate_created_authorization(
    created: &CreatedAuthorization,
    auth_base_url: &Url,
    callback_uri: &str,
) -> Result<()> {
    validate_token_shape(&created.device_code, "device secret")?;
    validate_token_shape(&created.state, "authorization state")?;
    if created.id.is_empty()
        || created.id.len() > 128
        || !created
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || created.user_code.is_empty()
        || created.user_code.len() > 32
        || !created
            .user_code
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
        || created.callback_uri != callback_uri
        || !DateTime::parse_from_rfc3339(&created.expires_at)
            .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > Utc::now())
    {
        return Err(AxiomError::Protocol(
            "Axiom returned invalid authorization metadata".into(),
        ));
    }
    let url = Url::parse(&created.authorization_url)
        .map_err(|_| AxiomError::Protocol("Axiom returned an invalid authorization URL".into()))?;
    if created.authorization_url.len() > 4_096
        || url.origin() != auth_base_url.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/native/authorize"
        || url.fragment().is_some()
        || url.query_pairs().count() != 2
        || !query_value_occurs_once(&url, "authorization_id", &created.id)
        || !query_value_occurs_once(&url, "state", &created.state)
    {
        return Err(AxiomError::Protocol(
            "Axiom returned an untrusted authorization URL".into(),
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
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AxiomError::Config(format!(
            "{label} URL must be an HTTPS origin (or loopback HTTP origin) with no credentials, path, query, or fragment"
        )));
    }
    Ok(url)
}

fn account_changed() -> AxiomError {
    AxiomError::SecureProvider {
        kind: axiom_inference::ProviderFailureKind::LocalAuthentication,
        code: "ACCOUNT_CHANGED",
        message: "the native account changed in another Axiom process; refresh account status",
    }
}

fn authentication_required() -> AxiomError {
    AxiomError::SecureProvider {
        kind: axiom_inference::ProviderFailureKind::LocalAuthentication,
        code: "AUTHENTICATION_REQUIRED",
        message: "authentication required; sign in to your Axiom account",
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

#[derive(Serialize)]
struct CreateAuthorizationRequest {
    code_challenge: String,
    callback_uri: String,
    client_id: String,
    client_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    method_hint: Option<LoginMethod>,
}

#[derive(Clone, Deserialize)]
struct CreatedAuthorization {
    id: String,
    device_code: String,
    user_code: String,
    state: String,
    authorization_url: String,
    callback_uri: String,
    expires_at: String,
    interval_seconds: u64,
}

#[derive(Serialize)]
struct ExchangeRequest<'a> {
    authorization_id: &'a str,
    device_code: &'a str,
    code_verifier: &'a str,
}

#[derive(Serialize)]
struct CancelAuthorizationRequest<'a> {
    device_code: &'a str,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    status: String,
    token_type: Option<String>,
    access_token: Option<String>,
    access_expires_at: Option<String>,
    refresh_token: Option<String>,
    refresh_expires_at: Option<String>,
    account: Option<AccountProfile>,
    session: Option<WireSession>,
}

impl ExchangeResponse {
    fn bundle(self) -> Option<SessionBundle> {
        let Self {
            token_type,
            access_token,
            access_expires_at,
            refresh_token,
            refresh_expires_at,
            account,
            session,
            ..
        } = self;
        let token_type = token_type?;
        let access_expires_at = access_expires_at?;
        let refresh_expires_at = refresh_expires_at?;
        session_bundle(
            &token_type,
            access_token?,
            &access_expires_at,
            refresh_token?,
            &refresh_expires_at,
            account?,
            session?,
        )
    }
}

#[derive(Deserialize)]
struct RefreshResponse {
    token_type: String,
    access_token: String,
    access_expires_at: String,
    refresh_token: String,
    refresh_expires_at: String,
    account: AccountProfile,
    session: WireSession,
}

impl RefreshResponse {
    fn bundle(self) -> Option<SessionBundle> {
        let Self {
            token_type,
            access_token,
            access_expires_at,
            refresh_token,
            refresh_expires_at,
            account,
            session,
        } = self;
        session_bundle(
            &token_type,
            access_token,
            &access_expires_at,
            refresh_token,
            &refresh_expires_at,
            account,
            session,
        )
    }
}

#[derive(Clone, Deserialize)]
struct WireSession {
    id: Option<String>,
    expires_at: String,
}

impl From<WireSession> for NativeSessionStatus {
    fn from(value: WireSession) -> Self {
        Self {
            id: value.id,
            expires_at: value.expires_at,
        }
    }
}

#[derive(Deserialize)]
struct SessionResponse {
    authenticated: bool,
    account: Option<AccountProfile>,
    session: Option<WireSession>,
}

struct SessionBundle {
    access_token: SecretString,
    access_expires_at: DateTime<Utc>,
    refresh_token: SecretString,
    account: AccountProfile,
    session: NativeSessionStatus,
}

fn session_bundle(
    token_type: &str,
    access_token: String,
    access_expires_at: &str,
    refresh_token: String,
    refresh_expires_at: &str,
    account: AccountProfile,
    session: WireSession,
) -> Option<SessionBundle> {
    let refresh_expires_at = DateTime::parse_from_rfc3339(refresh_expires_at)
        .ok()?
        .with_timezone(&Utc);
    let session_expires_at = DateTime::parse_from_rfc3339(&session.expires_at)
        .ok()?
        .with_timezone(&Utc);
    if !token_type.eq_ignore_ascii_case("bearer") || refresh_expires_at != session_expires_at {
        return None;
    }
    Some(SessionBundle {
        access_token: SecretString::from(access_token),
        access_expires_at: DateTime::parse_from_rfc3339(access_expires_at)
            .ok()?
            .with_timezone(&Utc),
        refresh_token: SecretString::from(refresh_token),
        account,
        session: session.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    };

    use axum::{
        Json, Router,
        extract::{Path as AxumPath, State},
        http::StatusCode as AxumStatus,
        response::{IntoResponse as _, Response},
        routing::{delete, get, post},
    };
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug)]
    struct MemoryStore {
        credential: Mutex<Option<String>>,
        client_id_path: PathBuf,
        refresh_lock_path: PathBuf,
        saves: AtomicUsize,
        deletes: AtomicUsize,
        fail_load: AtomicBool,
        fail_save: AtomicBool,
        fail_delete: AtomicBool,
        load_delay_ms: AtomicU64,
        save_delay_ms: AtomicU64,
        delete_delay_ms: AtomicU64,
    }

    impl CredentialStore for MemoryStore {
        fn source(&self) -> CredentialSource {
            CredentialSource::SystemKeyring
        }

        fn load(&self) -> std::result::Result<Option<String>, String> {
            std::thread::sleep(Duration::from_millis(
                self.load_delay_ms.load(Ordering::Acquire),
            ));
            if self.fail_load.load(Ordering::Acquire) {
                return Err("simulated locked keyring read failure".into());
            }
            Ok(self.credential.lock().expect("credential lock").clone())
        }

        fn save(&self, value: &str) -> std::result::Result<(), String> {
            std::thread::sleep(Duration::from_millis(
                self.save_delay_ms.load(Ordering::Acquire),
            ));
            self.saves.fetch_add(1, Ordering::AcqRel);
            if self.fail_save.load(Ordering::Acquire) {
                return Err("simulated keyring write failure".into());
            }
            *self.credential.lock().expect("credential lock") = Some(value.to_owned());
            Ok(())
        }

        fn delete(&self) -> std::result::Result<(), String> {
            std::thread::sleep(Duration::from_millis(
                self.delete_delay_ms.load(Ordering::Acquire),
            ));
            self.deletes.fetch_add(1, Ordering::AcqRel);
            if self.fail_delete.load(Ordering::Acquire) {
                return Err("simulated keyring delete failure".into());
            }
            *self.credential.lock().expect("credential lock") = None;
            Ok(())
        }

        fn client_id_path(&self) -> &Path {
            &self.client_id_path
        }

        fn refresh_lock_path(&self) -> &Path {
            &self.refresh_lock_path
        }
    }

    struct MockState {
        challenge: Mutex<Option<String>>,
        method_hint: Mutex<Option<String>>,
        auth_origin: Mutex<String>,
        approved: AtomicBool,
        refreshes: AtomicUsize,
        refresh_inputs: Mutex<Vec<String>>,
        refresh_failure: AtomicUsize,
        session_account_id: Mutex<Option<String>>,
        cancelled: AtomicBool,
        revocations: AtomicUsize,
        searches: AtomicUsize,
        search_mode: AtomicUsize,
        search_started: tokio::sync::Notify,
        gift_mode: AtomicUsize,
        gift_calls: AtomicUsize,
        gift_started: tokio::sync::Notify,
        gift_release: tokio::sync::Notify,
    }

    fn account_json() -> Value {
        json!({
            "id": "acct_test-123",
            "display_name": "Ada",
            "avatar_url": null,
            "verified_email": "ada@example.test",
            "linked_methods": ["passkey", "google"]
        })
    }

    fn bundle_json(access: &str, refresh: &str) -> Value {
        let access_expiry = (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339();
        let refresh_expiry = (Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        json!({
            "token_type": "bearer",
            "access_token": access,
            "access_expires_at": access_expiry,
            "refresh_token": refresh,
            "refresh_expires_at": refresh_expiry,
            "account": account_json(),
            "session": {"id": "session-1", "expires_at": refresh_expiry}
        })
    }

    async fn create_authorization(
        State(state): State<Arc<MockState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        *state.challenge.lock().expect("challenge lock") =
            body["code_challenge"].as_str().map(str::to_owned);
        *state.method_hint.lock().expect("method lock") =
            body["method_hint"].as_str().map(str::to_owned);
        let callback = body["callback_uri"].as_str().expect("callback");
        let authorization_url = format!(
            "{}/native/authorize?authorization_id=authorization-123&state=state-with-enough-entropy-123456789012345",
            state.auth_origin.lock().expect("auth origin lock")
        );
        Json(json!({
            "id": "authorization-123",
            "device_code": "device-code-with-enough-entropy-123456789",
            "user_code": "AXIOM-12",
            "state": "state-with-enough-entropy-123456789012345",
            "authorization_url": authorization_url,
            "callback_uri": callback,
            "expires_at": (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
            "interval_seconds": 1
        }))
    }

    async fn exchange_authorization(
        State(state): State<Arc<MockState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let verifier = body["code_verifier"].as_str().expect("verifier");
        let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(
            state.challenge.lock().expect("challenge lock").as_deref(),
            Some(actual.as_str())
        );
        if !state.approved.load(Ordering::Acquire) {
            return Json(json!({"status": "pending"}));
        }
        let mut result = bundle_json(
            "access-token-with-enough-entropy-123456789",
            "refresh-token-with-enough-entropy-12345678",
        );
        result["status"] = json!("complete");
        Json(result)
    }

    async fn refresh(State(state): State<Arc<MockState>>, Json(body): Json<Value>) -> Response {
        state
            .refresh_inputs
            .lock()
            .expect("refresh inputs lock")
            .push(
                body["refresh_token"]
                    .as_str()
                    .expect("refresh token")
                    .to_owned(),
            );
        let count = state.refreshes.fetch_add(1, Ordering::AcqRel) + 1;
        match state.refresh_failure.load(Ordering::Acquire) {
            1 => {
                return (
                    AxumStatus::INTERNAL_SERVER_ERROR,
                    Json(json!({"detail": "simulated response loss"})),
                )
                    .into_response();
            }
            2 => return Json(json!({"unexpected": true})).into_response(),
            3 => tokio::time::sleep(Duration::from_millis(75)).await,
            _ => {}
        }
        Json(bundle_json(
            &format!("access-token-with-enough-entropy-{count:08}"),
            &format!("refresh-token-with-enough-entropy-{count:08}"),
        ))
        .into_response()
    }

    async fn session(State(state): State<Arc<MockState>>) -> Json<Value> {
        let mut account = account_json();
        if let Some(id) = state
            .session_account_id
            .lock()
            .expect("session account lock")
            .as_ref()
        {
            account["id"] = json!(id);
        }
        Json(json!({
            "authenticated": true,
            "account": account,
            "session": {
                "id": "session-1",
                "expires_at": (Utc::now() + chrono::Duration::days(30)).to_rfc3339()
            }
        }))
    }

    async fn revoke_session(State(state): State<Arc<MockState>>) -> AxumStatus {
        state.revocations.fetch_add(1, Ordering::AcqRel);
        AxumStatus::NO_CONTENT
    }

    async fn cancel(
        State(state): State<Arc<MockState>>,
        AxumPath(_id): AxumPath<String>,
    ) -> AxumStatus {
        state.cancelled.store(true, Ordering::Release);
        AxumStatus::NO_CONTENT
    }

    async fn hosted_search(
        State(state): State<Arc<MockState>>,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        assert!(
            headers
                .get("authorization")
                .expect("bearer")
                .to_str()
                .expect("header")
                .starts_with("Bearer access-token-")
        );
        assert_eq!(body, json!({"query": "a concise query"}));
        state.searches.fetch_add(1, Ordering::AcqRel);
        state.search_started.notify_one();
        match state.search_mode.load(Ordering::Acquire) {
            1 => {
                return (
                    AxumStatus::TOO_MANY_REQUESTS,
                    [("retry-after", "17")],
                    "private body must never be displayed",
                )
                    .into_response();
            }
            2 => {
                return (
                    AxumStatus::FOUND,
                    [("location", "http://127.0.0.1:9/never-follow")],
                )
                    .into_response();
            }
            3 => return (AxumStatus::OK, "invalid response with private data").into_response(),
            4 => tokio::time::sleep(Duration::from_millis(150)).await,
            _ => {}
        }
        Json(json!({"provider": "decodo", "results": [{
            "title": "A result", "url": "https://example.com/", "snippet": "A snippet", "source": "decodo", "score": null
        }], "warnings": []})).into_response()
    }

    async fn api_key_control(request: axum::extract::Request) -> Response {
        assert!(
            request
                .headers()
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Bearer access-token-")
        );
        let method = request.method().clone();
        if method == axum::http::Method::DELETE {
            return AxumStatus::NO_CONTENT.into_response();
        }
        let key = json!({"id": "key-1", "name": "Coding tools", "scopes": ["inference"],
            "created_at": "2026-09-10T12:00:00Z", "last_used_at": null, "expires_at": null,
            "revoked_at": null, "usage_started_at": "2026-09-10T12:00:00Z",
            "usage": {"request_count": "1", "input_tokens": "12345", "cached_input_tokens": "1000",
                "output_tokens": "4000", "cost_microusd": "123456"}});
        if method == axum::http::Method::GET {
            return Json(json!({"keys": [key]})).into_response();
        }
        let body = axum::body::to_bytes(request.into_body(), 4096)
            .await
            .unwrap();
        let input: Value = serde_json::from_slice(&body).unwrap();
        if input["name"] == "Needs sign-in" {
            return (AxumStatus::FORBIDDEN, "private response details").into_response();
        }
        assert_eq!(input, json!({"name": "Coding tools"}));
        let mut created = key;
        created["token"] = "axm_once_only_fixture_1234567890".into();
        (AxumStatus::CREATED, Json(created)).into_response()
    }

    async fn manager() -> (AuthManager, Arc<MockState>, Arc<MemoryStore>) {
        let state = Arc::new(MockState {
            challenge: Mutex::new(None),
            method_hint: Mutex::new(None),
            auth_origin: Mutex::new(String::new()),
            approved: AtomicBool::new(false),
            refreshes: AtomicUsize::new(0),
            refresh_inputs: Mutex::new(Vec::new()),
            refresh_failure: AtomicUsize::new(0),
            session_account_id: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            revocations: AtomicUsize::new(0),
            searches: AtomicUsize::new(0),
            search_mode: AtomicUsize::new(0),
            search_started: tokio::sync::Notify::new(),
            gift_mode: AtomicUsize::new(0),
            gift_calls: AtomicUsize::new(0),
            gift_started: tokio::sync::Notify::new(),
            gift_release: tokio::sync::Notify::new(),
        });
        let app = Router::new()
            .route(
                "/api/v1/auth/native/authorizations",
                post(create_authorization),
            )
            .route("/api/v1/auth/native/authorizations/{id}", delete(cancel))
            .route("/api/v1/auth/native/exchange", post(exchange_authorization))
            .route("/api/v1/auth/token/refresh", post(refresh))
            .route("/api/v1/auth/session", get(session).delete(revoke_session))
            .route("/api/v1/search", post(hosted_search))
            .route("/api/v1/billing/gift-codes/redeem", post(gift_redeem_test))
            .route(
                "/api/v1/auth/api-keys",
                get(api_key_control).post(api_key_control),
            )
            .route("/api/v1/auth/api-keys/{id}", delete(api_key_control))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        base.clone_into(&mut state.auth_origin.lock().expect("auth origin lock"));
        tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
        let directory = tempfile::tempdir().expect("identity directory").keep();
        let store = Arc::new(MemoryStore {
            credential: Mutex::new(None),
            client_id_path: directory.join("client-id"),
            refresh_lock_path: directory.join("native-session.lock"),
            saves: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            fail_load: AtomicBool::new(false),
            fail_save: AtomicBool::new(false),
            fail_delete: AtomicBool::new(false),
            load_delay_ms: AtomicU64::new(0),
            save_delay_ms: AtomicU64::new(0),
            delete_delay_ms: AtomicU64::new(0),
        });
        let (client, api, _) = build_client(&base, Duration::from_secs(2)).expect("client");
        let mut auth = AuthManager::from_parts(
            store.clone(),
            client,
            api,
            Url::parse(&base).expect("auth URL"),
            AuthState {
                automation_key: None,
                refresh_token: None,
                access_token: None,
                account: None,
                session: None,
                source: store.source(),
                generation: 0,
                revision: 0,
                unavailable_detail: None,
                external_change_pending: false,
            },
        );
        auth.launch_browser = false;
        (auth, state, store)
    }

    #[tokio::test]
    async fn api_keys_use_native_auth_without_replacing_the_sign_in_credential() {
        let (auth, state, store) = manager().await;
        let login = auth.start_native_login(None).await.unwrap();
        state.approved.store(true, Ordering::Release);
        login.complete(CancellationToken::new()).await.unwrap();
        let credential = store.load().unwrap();
        let client = crate::billing::BillingClient::new(
            auth.api_base_url.as_str(),
            auth.clone(),
            Duration::from_secs(2),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let created = client
            .create_api_key("Coding tools", &cancellation)
            .await
            .unwrap();
        assert_eq!(created.token, "axm_once_only_fixture_1234567890");
        let listed = client.api_keys(&cancellation).await.unwrap();
        assert_eq!(listed.keys[0].usage.cost_microusd, "123456");
        assert!(!serde_json::to_string(&listed).unwrap().contains("axm_"));
        client
            .revoke_api_key(&created.key.id, &cancellation)
            .await
            .unwrap();
        let error = client
            .create_api_key("Needs sign-in", &cancellation)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Sign in again"));
        assert!(!error.contains("private response details"));
        assert_eq!(store.load().unwrap(), credential);
    }

    #[tokio::test]
    async fn hosted_search_uses_native_auth_with_bounded_queries_and_no_retry_or_redirect() {
        use crate::web::{HostedSearchClient, SearchProvider as _};
        let (auth, state, _) = manager().await;
        let search = HostedSearchClient::new(auth.clone()).expect("search client");
        assert!(
            search
                .search("a concise query", CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(state.searches.load(Ordering::Acquire), 0);
        let login = auth.start_native_login(None).await.expect("login");
        state.approved.store(true, Ordering::Release);
        login
            .complete(CancellationToken::new())
            .await
            .expect("signed in");
        assert!(
            search
                .search(&"x".repeat(2049), CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(state.searches.load(Ordering::Acquire), 0);
        let report = search
            .search(" a concise query ", CancellationToken::new())
            .await
            .expect("results");
        assert_eq!(report.results[0].url.as_str(), "https://example.com/");
        assert_eq!(report.results[0].source, "decodo");
        for mode in 1..=3 {
            state.search_mode.store(mode, Ordering::Release);
            let error = search
                .search("a concise query", CancellationToken::new())
                .await
                .expect_err("failure")
                .to_string();
            assert!(!error.contains("private"));
            assert!(!error.contains("access-token"));
            if mode == 1 {
                assert!(error.contains("17 seconds"));
            }
        }
        assert_eq!(
            state.searches.load(Ordering::Acquire),
            4,
            "one request per call, no paid retries"
        );
    }

    #[tokio::test]
    async fn hosted_search_discards_results_after_account_change_and_supports_cancellation() {
        use crate::web::{HostedSearchClient, SearchProvider as _};
        let (auth, state, _) = manager().await;
        let login = auth.start_native_login(None).await.expect("login");
        state.approved.store(true, Ordering::Release);
        login
            .complete(CancellationToken::new())
            .await
            .expect("signed in");
        let search = HostedSearchClient::new(auth.clone()).expect("client");
        state.search_mode.store(4, Ordering::Release);
        let pending = search.search("a concise query", CancellationToken::new());
        let change_account = async {
            state.search_started.notified().await;
            auth.shared.state.write().expect("state").generation += 1;
        };
        let (result, ()) = tokio::join!(pending, change_account);
        assert!(matches!(result, Err(AxiomError::Cancelled)));
        let cancellation = CancellationToken::new();
        let pending = search.search("a concise query", cancellation.clone());
        let cancel = async {
            state.search_started.notified().await;
            cancellation.cancel();
        };
        let (result, ()) = tokio::join!(pending, cancel);
        assert!(matches!(result, Err(AxiomError::Cancelled)));
    }

    fn peer_manager(
        manager: &AuthManager,
        store: &Arc<MemoryStore>,
        refresh_token: &str,
    ) -> AuthManager {
        let mut peer = AuthManager::from_parts(
            store.clone(),
            manager.client.clone(),
            manager.api_base_url.clone(),
            manager.auth_base_url.clone(),
            AuthState {
                automation_key: None,
                refresh_token: Some(SecretString::from(refresh_token.to_owned())),
                access_token: None,
                account: None,
                session: None,
                source: store.source(),
                generation: 0,
                revision: 0,
                unavailable_detail: None,
                external_change_pending: false,
            },
        );
        peer.launch_browser = false;
        peer
    }

    fn stalled_credential_delay_ms() -> u64 {
        u64::try_from((CREDENTIAL_OPERATION_TIMEOUT * 3).as_millis())
            .expect("credential test timeout fits in milliseconds")
    }

    #[test]
    fn secrets_are_redacted_from_debug() {
        let state = AuthState {
            automation_key: None,
            refresh_token: Some(SecretString::from("refresh-super-secret".to_owned())),
            access_token: Some(CachedAccessToken {
                token: SecretString::from("access-super-secret".to_owned()),
                expires_at: Utc::now(),
            }),
            account: None,
            session: None,
            source: CredentialSource::SystemKeyring,
            generation: 0,
            revision: 0,
            unavailable_detail: None,
            external_change_pending: false,
        };
        let text = format!("{state:?}");
        assert!(!text.contains("super-secret"));
        assert!(text.contains("REDACTED"));
    }

    #[tokio::test]
    async fn desktop_login_delegates_browser_launch_and_preserves_native_authorization() {
        let (mut standalone, state, store) = manager().await;
        standalone.launch_browser = true;
        let desktop = standalone.clone().without_browser_launch();
        assert!(standalone.launch_browser);
        assert!(!desktop.launch_browser);
        let login = desktop
            .start_native_login(Some(LoginMethod::Google))
            .await
            .expect("start desktop login");
        assert!(!login.browser_opened());
        assert!(store.load().expect("load").is_none());
        assert_eq!(
            state.method_hint.lock().expect("method lock").as_deref(),
            Some("google")
        );
        state.approved.store(true, Ordering::Release);
        let account = tokio::time::timeout(
            Duration::from_secs(2),
            login.complete(CancellationToken::new()),
        )
        .await
        .expect("login timeout")
        .expect("complete desktop login");
        assert_eq!(account.account.id, "acct_test-123");
        assert!(store.load().expect("load").is_some());
    }

    #[tokio::test]
    async fn native_login_preserves_tls_failure_cause_without_request_url() {
        let (mut auth, _, store) = manager().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let base = format!("https://{}", listener.local_addr().expect("address"));
        auth.api_base_url = Url::parse(&base).expect("API URL");
        auth.client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("connection");
            let mut hello = [0; 8192];
            assert!(socket.read(&mut hello).await.expect("TLS client hello") > 0);
            // Reproduce a network intermediary answering TLS with plaintext.
            socket
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("plaintext reply");
        });
        let error = auth
            .start_native_login(None)
            .await
            .expect_err("plaintext is not a TLS response")
            .to_string();
        server.await.expect("server task");
        assert!(error.contains("could not start login"), "{error}");
        assert!(error.contains("InvalidContentType"), "{error}");
        assert!(!error.contains(&base), "request URL must not be reflected");
        assert!(store.load().expect("load").is_none());
    }

    #[tokio::test]
    async fn native_login_uses_pkce_and_persists_only_the_refresh_token() {
        let (auth, state, store) = manager().await;
        let login = auth
            .start_native_login(Some(LoginMethod::Passkey))
            .await
            .expect("start login");
        assert!(!login.browser_opened());
        assert_eq!(login.user_code(), "AXIOM-12");
        let login_debug = format!("{login:?}");
        assert!(login_debug.contains("authorization_url: \"[REDACTED]\""));
        assert!(!login_debug.contains("state-with-enough-entropy"));
        assert!(!login_debug.contains("device-code-with-enough-entropy"));
        assert_eq!(
            state.method_hint.lock().expect("method lock").as_deref(),
            Some("passkey")
        );
        assert!(store.load().expect("load").is_none());
        state.approved.store(true, Ordering::Release);
        let account = tokio::time::timeout(
            Duration::from_secs(2),
            login.complete(CancellationToken::new()),
        )
        .await
        .expect("login timeout")
        .expect("login");
        assert_eq!(account.account.id, "acct_test-123");
        assert_eq!(
            store.load().expect("load").as_deref(),
            Some("refresh-token-with-enough-entropy-12345678")
        );
        let debug = format!("{auth:?}");
        assert!(!debug.contains("refresh-token"));
        assert!(!debug.contains("access-token"));
    }

    #[tokio::test]
    async fn concurrent_access_requests_rotate_the_refresh_token_once() {
        let (auth, state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh_token.to_owned()));
        let one = auth.clone();
        let two = auth.clone();
        let first_cancel = CancellationToken::new();
        let second_cancel = CancellationToken::new();
        let (first, second) = tokio::join!(
            one.access_token(Duration::from_secs(60), &first_cancel),
            two.access_token(Duration::from_secs(60), &second_cancel),
        );
        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(state.refreshes.load(Ordering::Acquire), 1);
        assert_eq!(store.saves.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn independent_managers_serialize_rotation_and_adopt_the_replacement() {
        let (first, server, store) = manager().await;
        let initial_refresh = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(initial_refresh.into());
        first
            .shared
            .state
            .write()
            .expect("state lock")
            .refresh_token = Some(SecretString::from(initial_refresh.to_owned()));
        let second = peer_manager(&first, &store, initial_refresh);

        let (first_status, second_status) = tokio::join!(first.validate(), second.validate());

        assert!(matches!(first_status, ValidationStatus::Valid(_)));
        assert!(matches!(second_status, ValidationStatus::Valid(_)));
        assert_eq!(server.refreshes.load(Ordering::Acquire), 2);
        assert_eq!(
            *server.refresh_inputs.lock().expect("refresh inputs lock"),
            vec![
                initial_refresh.to_owned(),
                "refresh-token-with-enough-entropy-00000001".to_owned(),
            ],
            "the second manager must reload the rotated token under the process-shared lock",
        );
        assert_eq!(
            store.load().expect("load").as_deref(),
            Some("refresh-token-with-enough-entropy-00000002")
        );
    }

    #[tokio::test]
    async fn peer_rotation_preserves_account_identity_and_live_operation_leases() {
        let (first, server, store) = manager().await;
        let initial = "initial-refresh-token-with-enough-entropy";
        store.save(initial).unwrap();
        first.shared.state.write().unwrap().refresh_token = Some(initial.to_owned().into());
        let second = peer_manager(&first, &store, initial);
        let ValidationStatus::Valid(status) = first.validate().await else {
            panic!("first account")
        };
        assert!(matches!(
            second.validate().await,
            ValidationStatus::Valid(_)
        ));
        first.shared.state.write().unwrap().access_token = None;
        let generation = first.generation();
        let first_cancel = CancellationToken::new();
        let second_cancel = CancellationToken::new();
        let (one, two) = tokio::join!(
            first.access_token_for_secure_operation(&first_cancel),
            first.access_token_for_secure_operation(&second_cancel),
        );
        assert!(one.is_ok() && two.is_ok());
        assert_eq!(server.refreshes.load(Ordering::Acquire), 3);
        assert_eq!(first.generation(), generation);
        assert!(first.account_status_is_current(&status));
        assert_eq!(server.revocations.load(Ordering::Acquire), 0);
        second.shared.state.write().unwrap().access_token = None;
        assert!(matches!(
            second.validate().await,
            ValidationStatus::Valid(_)
        ));
        first.shared.state.write().unwrap().access_token = None;
        assert!(matches!(first.validate().await, ValidationStatus::Valid(_)));
        assert_eq!(
            first.generation(),
            generation,
            "account refresh also preserves identity"
        );
        assert!(first.account_status_is_current(&status));
    }

    #[tokio::test]
    async fn peer_account_switch_preserves_credential_but_blocks_existing_work() {
        let (auth, server, store) = manager().await;
        let initial = "initial-refresh-token-with-enough-entropy";
        store.save(initial).unwrap();
        auth.shared.state.write().unwrap().refresh_token = Some(initial.to_owned().into());
        assert!(matches!(auth.validate().await, ValidationStatus::Valid(_)));
        {
            let mut state = auth.shared.state.write().unwrap();
            state.account.as_mut().unwrap().id = "previous-account".into();
            state.access_token = None;
        }
        let generation = auth.generation();
        store
            .save("peer-login-refresh-token-with-enough-entropy")
            .unwrap();
        let error = auth
            .access_token_for_secure_operation(&CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("account changed"));
        assert!(auth.generation() > generation);
        assert_eq!(server.revocations.load(Ordering::Acquire), 0);
        assert_eq!(
            store.load().unwrap().as_deref(),
            Some("refresh-token-with-enough-entropy-00000002")
        );
        assert!(
            auth.access_token_for_secure_operation(&CancellationToken::new())
                .await
                .is_err()
        );
        assert!(matches!(auth.validate().await, ValidationStatus::Valid(_)));
    }

    #[tokio::test]
    async fn stale_manager_never_deletes_a_newer_persisted_refresh_token() {
        let (auth, _server, store) = manager().await;
        let stale_refresh = "stale-refresh-token-with-enough-entropy";
        let replacement = "replacement-refresh-token-with-enough-entropy";
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(stale_refresh.to_owned()));
        *store.credential.lock().expect("credential lock") = Some(replacement.to_owned());

        auth.delete_persisted_and_clear()
            .await
            .expect("stale local state can be cleared");

        assert_eq!(store.load().expect("load").as_deref(), Some(replacement));
        assert_eq!(store.deletes.load(Ordering::Acquire), 0);
        assert!(!auth.has_credential());
    }

    #[tokio::test]
    async fn distinct_api_origins_never_share_send_or_delete_refresh_credentials() {
        let (first, first_server, first_store) = manager().await;
        let (second, second_server, second_store) = manager().await;
        let first_refresh = "first-origin-refresh-token-with-enough-entropy";
        let second_refresh = "second-origin-refresh-token-with-enough-entropy";
        *first_store.credential.lock().expect("credential lock") = Some(first_refresh.into());
        *second_store.credential.lock().expect("credential lock") = Some(second_refresh.into());
        first
            .shared
            .state
            .write()
            .expect("state lock")
            .refresh_token = Some(SecretString::from(first_refresh.to_owned()));
        second
            .shared
            .state
            .write()
            .expect("state lock")
            .refresh_token = Some(SecretString::from(second_refresh.to_owned()));

        let common_identity = PathBuf::from("platform-data/axiom/client-id");
        let (first_account, first_lock) = credential_scope(&first.api_base_url, &common_identity);
        let (second_account, second_lock) =
            credential_scope(&second.api_base_url, &common_identity);
        assert_ne!(first_account, second_account);
        assert_ne!(first_lock, second_lock);
        assert!(!first_account.contains(first.api_base_url.host_str().expect("host")));
        assert!(!first_lock.to_string_lossy().contains(':'));

        let first_cancellation = CancellationToken::new();
        let second_cancellation = CancellationToken::new();
        let (first_lease, second_lease) = tokio::join!(
            first.access_token(Duration::ZERO, &first_cancellation),
            second.access_token(Duration::ZERO, &second_cancellation),
        );
        assert!(first_lease.is_ok());
        assert!(second_lease.is_ok());
        assert_eq!(
            *first_server
                .refresh_inputs
                .lock()
                .expect("first refresh inputs"),
            vec![first_refresh.to_owned()]
        );
        assert_eq!(
            *second_server
                .refresh_inputs
                .lock()
                .expect("second refresh inputs"),
            vec![second_refresh.to_owned()]
        );
        let second_persisted = second_store.load().expect("second load");
        first
            .delete_persisted_and_clear()
            .await
            .expect("delete first audience");
        assert_eq!(second_store.load().expect("second load"), second_persisted);
        assert_eq!(second_store.deletes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn ambiguous_refresh_results_burn_the_local_token_instead_of_replaying_it() {
        for failure in [1, 2] {
            let (auth, state, store) = manager().await;
            let refresh_token = "initial-refresh-token-with-enough-entropy";
            *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
            auth.shared.state.write().expect("state lock").refresh_token =
                Some(SecretString::from(refresh_token.to_owned()));
            state.refresh_failure.store(failure, Ordering::Release);

            let error = auth
                .access_token(Duration::ZERO, &CancellationToken::new())
                .await
                .expect_err("an ambiguous refresh result must require a fresh login");

            assert!(!error.to_string().contains(refresh_token));
            assert!(!auth.has_credential());
            assert!(store.load().expect("load").is_none());
            assert_eq!(store.deletes.load(Ordering::Acquire), 1);
            assert_eq!(state.refreshes.load(Ordering::Acquire), 1);
        }
    }

    #[tokio::test]
    async fn dropping_a_caller_after_refresh_dispatch_still_commits_the_rotation() {
        let (auth, state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh_token.to_owned()));
        state.refresh_failure.store(3, Ordering::Release);

        let caller_auth = auth.clone();
        let caller = tokio::spawn(async move {
            caller_auth
                .access_token(Duration::ZERO, &CancellationToken::new())
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.refreshes.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("refresh was dispatched");
        caller.abort();
        let _ = caller.await;

        tokio::time::timeout(Duration::from_secs(1), async {
            while store
                .load()
                .expect("load")
                .is_none_or(|token| token == refresh_token)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached rotation committed to the credential store");
        assert_eq!(state.refreshes.load(Ordering::Acquire), 1);
        assert_eq!(store.saves.load(Ordering::Acquire), 1);
        assert!(
            store
                .load()
                .expect("load")
                .is_some_and(|token| token != refresh_token)
        );
        assert!(auth.has_credential());
    }

    #[tokio::test]
    async fn refresh_persistence_failure_revokes_and_clears_every_local_secret() {
        let (auth, state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh_token.to_owned()));
        store.fail_save.store(true, Ordering::Release);

        let error = auth
            .access_token(Duration::ZERO, &CancellationToken::new())
            .await
            .expect_err("a rotated token that cannot reach the keyring must fail closed");
        assert!(error.to_string().contains("system credential store"));
        assert!(!auth.has_credential());
        assert!(store.load().expect("load").is_none());
        assert_eq!(store.deletes.load(Ordering::Acquire), 1);
        assert_eq!(state.revocations.load(Ordering::Acquire), 1);
        assert!(matches!(auth.validate().await, ValidationStatus::Missing));
    }

    #[tokio::test]
    async fn logout_refreshes_before_server_revocation_when_access_is_not_cached() {
        let (auth, state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh_token.to_owned()));

        auth.logout_async().await.expect("logout");

        assert_eq!(state.refreshes.load(Ordering::Acquire), 1);
        assert_eq!(state.revocations.load(Ordering::Acquire), 1);
        assert!(!auth.has_credential());
        assert!(store.load().expect("load").is_none());
    }

    #[tokio::test]
    async fn keyring_delete_failure_still_clears_in_memory_authorization() {
        let (auth, _state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh_token.to_owned()));
        store.fail_delete.store(true, Ordering::Release);

        let error = auth
            .logout_async()
            .await
            .expect_err("keyring deletion must be reported");

        assert!(error.to_string().contains("keyring delete failure"));
        assert!(!auth.has_credential());
        assert!(auth.active_account_id().is_none());
        assert!(auth.unavailable_detail().is_some());
        assert!(!format!("{auth:?}").contains("refresh-token-with-enough-entropy"));
    }

    #[tokio::test]
    async fn locked_credential_coordination_times_out_and_disables_local_auth() {
        let (auth, _state, store) = manager().await;
        let held = acquire_refresh_file_lock(store.refresh_lock_path())
            .expect("hold audience coordination lock");

        let error = auth
            .acquire_credential_lock()
            .await
            .expect_err("a locked audience must have a bounded wait");
        assert!(error.to_string().contains(CREDENTIAL_TIMEOUT_MARKER));
        assert!(auth.unavailable_detail().is_some());

        drop(held);
        tokio::time::sleep(CREDENTIAL_OPERATION_TIMEOUT * 2).await;
    }

    #[tokio::test]
    async fn locked_keyring_read_fails_closed_without_deleting_an_uncompared_token() {
        let (auth, state, store) = manager().await;
        let refresh = "locked-keyring-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh.to_owned()));
        store.fail_load.store(true, Ordering::Release);

        let error = auth
            .access_token(Duration::ZERO, &CancellationToken::new())
            .await
            .expect_err("a denied keyring read must fail closed");
        assert!(error.to_string().contains("locked keyring read failure"));
        assert!(!auth.has_credential());
        assert!(auth.unavailable_detail().is_some());
        assert_eq!(state.refreshes.load(Ordering::Acquire), 0);
        assert_eq!(store.deletes.load(Ordering::Acquire), 0);

        store.fail_load.store(false, Ordering::Release);
        assert_eq!(store.load().expect("persisted token"), Some(refresh.into()));
    }

    #[tokio::test]
    async fn credential_read_timeout_fails_closed_without_losing_the_persisted_token() {
        let (auth, _state, store) = manager().await;
        let refresh = "read-timeout-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh.to_owned()));
        store
            .load_delay_ms
            .store(stalled_credential_delay_ms(), Ordering::Release);

        let error = auth
            .access_token(Duration::ZERO, &CancellationToken::new())
            .await
            .expect_err("stalled credential read must time out");
        assert!(error.to_string().contains(CREDENTIAL_TIMEOUT_MARKER));
        assert!(!auth.has_credential());
        assert!(auth.unavailable_detail().is_some());

        tokio::time::sleep(CREDENTIAL_OPERATION_TIMEOUT * 3).await;
        store.load_delay_ms.store(0, Ordering::Release);
        assert_eq!(store.load().expect("persisted token"), Some(refresh.into()));
    }

    #[tokio::test]
    async fn credential_save_timeout_revokes_and_never_races_a_cleanup_delete() {
        let (auth, state, store) = manager().await;
        let refresh = "save-timeout-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh.into());
        auth.shared.state.write().expect("state lock").refresh_token =
            Some(SecretString::from(refresh.to_owned()));
        store
            .save_delay_ms
            .store(stalled_credential_delay_ms(), Ordering::Release);

        let error = auth
            .access_token(Duration::ZERO, &CancellationToken::new())
            .await
            .expect_err("stalled rotated-token save must fail closed");
        assert!(error.to_string().contains(CREDENTIAL_TIMEOUT_MARKER));
        assert!(!auth.has_credential());
        assert!(auth.unavailable_detail().is_some());
        assert_eq!(state.revocations.load(Ordering::Acquire), 1);

        tokio::time::sleep(CREDENTIAL_OPERATION_TIMEOUT * 5).await;
        store.save_delay_ms.store(0, Ordering::Release);
        assert!(store.load().expect("late-save cleanup").is_none());
        assert_eq!(store.deletes.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn credential_delete_timeout_clears_memory_and_finishes_under_the_audience_lock() {
        let (auth, state, store) = manager().await;
        let refresh = "delete-timeout-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh.into());
        {
            let mut local = auth.shared.state.write().expect("state lock");
            local.refresh_token = Some(SecretString::from(refresh.to_owned()));
            local.access_token = Some(CachedAccessToken {
                token: SecretString::from("cached-access-token-with-enough-entropy".to_owned()),
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            });
        }
        store
            .delete_delay_ms
            .store(stalled_credential_delay_ms(), Ordering::Release);

        let error = auth
            .logout_async()
            .await
            .expect_err("stalled credential deletion must be reported");
        assert!(error.to_string().contains(CREDENTIAL_TIMEOUT_MARKER));
        assert!(!auth.has_credential());
        assert!(auth.unavailable_detail().is_some());
        assert_eq!(state.revocations.load(Ordering::Acquire), 1);

        let peer_error = auth
            .acquire_credential_lock()
            .await
            .expect_err("late delete must retain the audience lock");
        assert!(peer_error.to_string().contains(CREDENTIAL_TIMEOUT_MARKER));
        tokio::time::sleep(CREDENTIAL_OPERATION_TIMEOUT * 5).await;
        store.delete_delay_ms.store(0, Ordering::Release);
        assert!(store.load().expect("late delete").is_none());
        let replacement = "replacement-after-delete-with-enough-entropy";
        // A fresh owner can safely save only after the supervised late delete
        // has released the audience lock.
        *store.credential.lock().expect("credential lock") = Some(replacement.into());
        tokio::time::sleep(CREDENTIAL_OPERATION_TIMEOUT).await;
        assert_eq!(store.load().expect("replacement"), Some(replacement.into()));
    }

    #[tokio::test]
    async fn refresh_cannot_silently_switch_the_account_bound_to_local_storage() {
        let (auth, state, store) = manager().await;
        let old_refresh = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(old_refresh.into());
        {
            let mut auth_state = auth.shared.state.write().expect("state lock");
            auth_state.refresh_token = Some(SecretString::from(old_refresh.to_owned()));
            auth_state.account =
                Some(serde_json::from_value(account_json()).expect("existing account profile"));
        }
        let mut rotated = bundle_json(
            "new-access-token-with-enough-entropy-12345",
            "new-refresh-token-with-enough-entropy-1234",
        );
        rotated["account"]["id"] = json!("different-account");
        let bundle = serde_json::from_value::<RefreshResponse>(rotated)
            .expect("refresh response")
            .bundle()
            .expect("session bundle");

        let error = auth
            .commit_bundle(bundle, false)
            .await
            .expect_err("refresh must remain bound to one account");

        assert!(error.to_string().contains("changed accounts"));
        assert!(!auth.has_credential());
        assert!(store.load().expect("load").is_none());
        assert_eq!(state.revocations.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn session_validation_cannot_relabel_an_existing_account() {
        let (auth, state, store) = manager().await;
        let refresh_token = "initial-refresh-token-with-enough-entropy";
        *store.credential.lock().expect("credential lock") = Some(refresh_token.into());
        {
            let mut auth_state = auth.shared.state.write().expect("state lock");
            auth_state.refresh_token = Some(SecretString::from(refresh_token.to_owned()));
            auth_state.access_token = Some(CachedAccessToken {
                token: SecretString::from("existing-access-token-with-enough-entropy".to_owned()),
                expires_at: Utc::now() + chrono::Duration::minutes(15),
            });
            auth_state.account =
                Some(serde_json::from_value(account_json()).expect("existing account profile"));
        }
        *state
            .session_account_id
            .lock()
            .expect("session account lock") = Some("different-account".into());

        let result = auth.validate().await;

        assert!(matches!(result, ValidationStatus::Expired));
        assert!(!auth.has_credential());
        assert!(auth.active_account_id().is_none());
        assert!(store.load().expect("load").is_none());
    }

    #[tokio::test]
    async fn cancellation_is_headless_and_uses_the_one_time_device_secret() {
        let (auth, state, _) = manager().await;
        let login = auth.start_native_login(None).await.expect("start login");
        login.cancel().await.expect("cancel login");
        assert!(state.cancelled.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn cancelling_completion_cancels_the_server_authorization_without_launching_ui() {
        let (auth, state, _) = manager().await;
        let login = auth.start_native_login(None).await.expect("start login");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            login.complete(cancellation).await,
            Err(AxiomError::Cancelled)
        ));
        assert!(state.cancelled.load(Ordering::Acquire));
    }

    async fn send_loopback_request(request: &str) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let expected_host = address.to_string();
        let request = request.replace("EXPECTED_HOST", &expected_host);
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.expect("connect");
            stream.write_all(request.as_bytes()).await.expect("write");
            stream.shutdown().await.expect("shutdown");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("read");
            response
        });
        let (mut socket, _) = listener.accept().await.expect("accept");
        let accepted = handle_loopback_request(
            &mut socket,
            "https://auth.axiom.stream",
            &expected_host,
            "authorization-123",
            "expected-state-with-enough-entropy",
        )
        .await
        .expect("callback");
        drop(socket);
        let response = client.await.expect("client");
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("Content-Security-Policy: default-src 'none'"));
        assert!(!response.contains("authorization-123"));
        assert!(!response.contains("expected-state-with-enough-entropy"));
        if accepted {
            assert!(response.contains("Back to your space."));
        } else {
            assert!(response.contains("This Axiom callback was rejected."));
        }
        accepted
    }

    #[tokio::test]
    async fn loopback_requires_exact_method_host_origin_id_and_state() {
        let valid = concat!(
            "GET /callback?authorization_id=authorization-123&state=expected-state-with-enough-entropy HTTP/1.1\r\n",
            "Host: EXPECTED_HOST\r\n",
            "Origin: https://auth.axiom.stream\r\n\r\n",
        );
        assert!(send_loopback_request(valid).await);
        assert!(
            send_loopback_request(&valid.replace("Origin: https://auth.axiom.stream\r\n", "",))
                .await,
            "top-level system-browser navigation may omit Origin"
        );
        for invalid in [
            valid.replacen("GET", "POST", 1),
            valid.replace("EXPECTED_HOST", "127.0.0.1:1"),
            valid.replace("auth.axiom.stream", "evil.example"),
            valid.replace("expected-state-with-enough-entropy", "wrong-state"),
            valid.replace(
                "state=expected-state-with-enough-entropy",
                "state=expected-state-with-enough-entropy&state=expected-state-with-enough-entropy",
            ),
            valid.replace(
                "Origin: https://auth.axiom.stream\r\n",
                "Origin: https://evil.example\r\nOrigin: https://auth.axiom.stream\r\n",
            ),
        ] {
            assert!(!send_loopback_request(&invalid).await);
        }
    }

    #[test]
    fn service_urls_and_automation_keys_are_bounded() {
        assert!(validate_api_key_shape("axm_1234567890123456").is_ok());
        assert!(validate_api_key_shape("not-a-key").is_err());
        assert!(parse_service_url("https://api.axiom.stream", "API").is_ok());
        assert!(parse_service_url("http://127.0.0.1:8000", "API").is_ok());
        assert!(parse_service_url("http://example.com", "API").is_err());
        for unsafe_url in [
            "https://api.axiom.stream/v1",
            "https://api.axiom.stream/?query=value",
            "https://api.axiom.stream/#fragment",
            "https://user@api.axiom.stream",
            "https://user:password@api.axiom.stream",
        ] {
            assert!(parse_service_url(unsafe_url, "API").is_err());
        }
        assert_eq!(
            serde_json::to_value(LoginMethod::EthereumWallet).expect("serialize login method"),
            json!("ethereum")
        );
        assert_eq!(
            serde_json::from_value::<LoginMethod>(json!("ethereum"))
                .expect("deserialize login method"),
            LoginMethod::EthereumWallet
        );
        assert!(
            serde_json::from_value::<LoginMethod>(json!("ethereum_wallet")).is_err(),
            "the removed backend wire alias must not remain accepted"
        );
    }

    #[test]
    fn authorization_and_account_metadata_cannot_inject_paths_or_unsafe_urls() {
        let expiry = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let valid = CreatedAuthorization {
            id: "authorization-123".into(),
            device_code: "device-code-with-enough-entropy-123456789".into(),
            user_code: "AXIOM-12".into(),
            state: "state-with-enough-entropy-123456789012345".into(),
            authorization_url: concat!(
                "https://auth.axiom.stream/native/authorize?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345"
            )
            .into(),
            callback_uri: "http://127.0.0.1:32123/callback".into(),
            expires_at: expiry,
            interval_seconds: 1,
        };
        let auth_origin = Url::parse("https://auth.axiom.stream").expect("URL");
        assert!(
            validate_created_authorization(
                &valid,
                &auth_origin,
                "http://127.0.0.1:32123/callback",
            )
            .is_ok()
        );

        let mut invalid_id = valid.clone();
        invalid_id.id = "../session".into();
        invalid_id.authorization_url = concat!(
            "https://auth.axiom.stream/native/authorize?authorization_id=../session&",
            "state=state-with-enough-entropy-123456789012345"
        )
        .into();
        assert!(
            validate_created_authorization(
                &invalid_id,
                &auth_origin,
                "http://127.0.0.1:32123/callback",
            )
            .is_err()
        );

        for unsafe_url in [
            concat!(
                "https://auth.axiom.stream/native/other?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345"
            ),
            concat!(
                "https://auth.axiom.stream:444/native/authorize?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345"
            ),
            concat!(
                "https://user@auth.axiom.stream/native/authorize?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345"
            ),
            concat!(
                "https://auth.axiom.stream/native/authorize?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345#fragment"
            ),
            concat!(
                "https://auth.axiom.stream/native/authorize?authorization_id=authorization-123&",
                "state=state-with-enough-entropy-123456789012345&next=https://evil.example"
            ),
            concat!(
                "https://auth.axiom.stream/native/authorize?authorization_id=authorization-123&",
                "authorization_id=authorization-123&state=state-with-enough-entropy-123456789012345"
            ),
        ] {
            let mut created = valid.clone();
            created.authorization_url = unsafe_url.into();
            assert!(
                validate_created_authorization(
                    &created,
                    &auth_origin,
                    "http://127.0.0.1:32123/callback",
                )
                .is_err(),
                "unsafe authorization URL was accepted: {unsafe_url}"
            );
        }
        let mut account: AccountProfile = serde_json::from_value(account_json()).expect("account");
        account.avatar_url = Some("file:///tmp/tracking-pixel".into());
        assert!(validate_account(&account).is_err());
    }
    include!("auth/gift_code_tests.rs");
}
