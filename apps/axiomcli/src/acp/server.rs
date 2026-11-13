//! Stdio lifecycle and typed ACP registration.

use super::*;

pub(super) struct ServerContext {
    pub(super) runner: Arc<dyn TurnRunner>,
    pub(super) config: Config,
    pub(super) store: Option<SessionStore>,
    pub(super) auth: AuthManager,
    pub(super) frontend: FrontendKind,
    pub(super) sessions: Sessions,
    pub(super) account_work: Arc<AccountWorkTracker>,
    pub(super) account_transition: Arc<Mutex<()>>,
    pub(super) extension_state: Arc<ExtensionState>,
    pub(super) billing: BillingClient,
    pub(super) profile_settings_lock: Arc<Mutex<()>>,
    pub(super) persistence_failpoint: Arc<PersistenceFailpoint>,
    pub(super) runtime: Runtime,
    pub(super) default_model: String,
    pub(super) default_thinking: crate::app::ThinkingLevel,
    pub(super) supports_load: bool,
    pub(super) supports_elicitation: Arc<AtomicBool>,
}

// Keep each request typed while sharing one connection-scoped context.
macro_rules! request_handler {
    (@call sync, $handler:path, $context:expr, $request:expr, $responder:expr, $connection:expr) => { $handler($context, $request, $responder, &$connection) };
    (@call sync_ref, $handler:path, $context:expr, $request:expr, $responder:expr, $connection:expr) => { $handler($context, &$request, $responder, &$connection) };
    (@call async, $handler:path, $context:expr, $request:expr, $responder:expr, $connection:expr) => { $handler($context, $request, $responder, &$connection).await };
    ($mode:ident, $builder:expr, $context:expr, $module:ident::$handler:ident, $request:ty, $response:ty) => {{
        let context = Arc::clone(&$context);
        $builder.on_receive_request(
            async move |request: $request, responder: Responder<$response>, connection| {
                request_handler!(@call $mode, $module::$handler, &context, request, responder, connection)
            },
            agent_client_protocol::on_receive_request!(),
        )
    }};
}

/// Serve ACP v1 over stdio. Stdout is owned by the protocol transport.
pub async fn serve_stdio(
    runner: Arc<dyn TurnRunner>,
    config: Config,
    store: Option<SessionStore>,
    auth: AuthManager,
    frontend: FrontendKind,
) -> agent_client_protocol::Result<()> {
    // Electron validates and opens desktop sign-in URLs in the user's actual
    // desktop session. The CLI's isolated staging storage must not select a
    // different browser or browser profile.
    let auth = if frontend == FrontendKind::DesktopChat {
        auth.without_browser_launch()
    } else {
        auth
    };
    let sessions: Sessions = Arc::new(Mutex::new(SessionRegistry::default()));
    let account_work = Arc::new(AccountWorkTracker::default());
    let account_transition = Arc::new(Mutex::new(()));
    let extension_state = Arc::new(ExtensionState::new());
    let billing = BillingClient::new(&config.base_url, auth.clone(), config.request_timeout())
        .map_err(agent_error)?;
    let profile_settings_lock = Arc::new(Mutex::new(()));
    let persistence_failpoint = Arc::new(PersistenceFailpoint::from_environment());
    let runtime = Runtime::new(512);
    let default_model = config.model.clone();
    let default_thinking = crate::app::ThinkingLevel::Medium;

    let supports_load = store.is_some();
    let supports_elicitation = Arc::new(AtomicBool::new(false));
    let context = Arc::new(ServerContext {
        runner,
        config,
        store,
        auth,
        frontend,
        sessions,
        account_work,
        account_transition,
        extension_state,
        billing,
        profile_settings_lock,
        persistence_failpoint,
        runtime,
        default_model,
        default_thinking,
        supports_load,
        supports_elicitation,
    });
    let builder = Agent.builder().name("axiomcli");
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        sessions::initialize,
        protocol::InitializeRequest,
        protocol::InitializeResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        sessions::desktop_bootstrap,
        DesktopBootstrapRequest,
        DesktopBootstrapResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        sessions::new_session,
        protocol::NewSessionRequest,
        protocol::NewSessionResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        sessions::load_session,
        protocol::LoadSessionRequest,
        protocol::LoadSessionResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        prompts::prompt,
        protocol::PromptRequest,
        protocol::PromptResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        threads::list_threads,
        ListThreadsRequest,
        ListThreadsResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        threads::rename_thread,
        extension::RenameThreadRequest,
        extension::RenameThreadResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        threads::get_thread_timeline,
        GetThreadTimelineRequest,
        GetThreadTimelineResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        threads::get_attachments,
        extension::GetAttachmentsRequest,
        extension::GetAttachmentsResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        threads::delete_preview,
        DeletePreviewRequest,
        DeletePreviewResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        threads::delete_confirm,
        DeleteConfirmRequest,
        DeleteConfirmResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        catalog::list_models,
        ListModelsRequest,
        ListModelsResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        catalog::get_profile_preferences,
        extension::GetProfilePreferencesRequest,
        ProfilePreferencesResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        catalog::set_profile_preferences,
        extension::SetProfilePreferencesRequest,
        ProfilePreferencesResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        collections::list_collections,
        extension::ListCollectionsRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::create_collection,
        extension::CreateCollectionRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::rename_collection,
        extension::RenameCollectionRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::set_collection_collapsed,
        extension::SetCollectionCollapsedRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::move_collection,
        extension::MoveCollectionRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::delete_collection,
        extension::DeleteCollectionRequest,
        CollectionStateResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        collections::assign_thread_collection,
        extension::AssignThreadCollectionRequest,
        extension::AssignThreadCollectionResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::account_status,
        extension::AccountStatusRequest,
        extension::AccountStatusResponse
    );
    let builder = request_handler!(
        sync_ref,
        builder,
        context,
        account::native_login_start,
        NativeLoginStartRequest,
        NativeLoginStartResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::native_login_complete,
        NativeLoginCompleteRequest,
        extension::AccountStatusResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::native_login_cancel,
        NativeLoginCancelRequest,
        extension::AccountStatusResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::logout,
        LogoutRequest,
        extension::AccountStatusResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::api_key_list,
        extension::ApiKeyListRequest,
        extension::ApiKeyListResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::api_key_create,
        extension::ApiKeyCreateRequest,
        extension::ApiKeyCreatedResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::api_key_revoke,
        extension::ApiKeyRevokeRequest,
        extension::ApiKeyRevokeResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::usage_summary,
        extension::UsageSummaryRequest,
        extension::UsageSummaryResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::billing_status,
        BillingStatusRequest,
        BillingStatusResponse
    );
    let builder = request_handler!(
        sync,
        builder,
        context,
        account::redeem_gift_code,
        extension::GiftCodeRedeemRequest,
        extension::GiftCodeRedeemResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        security::verify_security,
        VerifySecurityRequest,
        VerifySecurityResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        security::prewarm_security,
        extension::PrewarmSecurityRequest,
        VerifySecurityResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        prompts::steer_turn,
        extension::SteerTurnRequest,
        extension::SteerTurnResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        security::compact,
        CompactRequest,
        CompactResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        settings::configure_desktop_agent,
        extension::ConfigureDesktopAgentRequest,
        extension::ConfigureDesktopAgentResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        settings::set_session_mode,
        protocol::SetSessionModeRequest,
        protocol::SetSessionModeResponse
    );
    let builder = request_handler!(
        async,
        builder,
        context,
        settings::set_session_config_option,
        protocol::SetSessionConfigOptionRequest,
        protocol::SetSessionConfigOptionResponse
    );
    let cancel_context = Arc::clone(&context);
    let builder = builder.on_receive_notification(
        async move |notification: protocol::CancelNotification, connection| {
            prompts::cancel(&cancel_context, notification, &connection).await
        },
        agent_client_protocol::on_receive_notification!(),
    );

    let result = Box::pin(builder.connect_to(ByteStreams::new(
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
    )))
    .await;
    // Drain owned processes while Tokio is alive; guard Drop handles aborted tasks.
    context.runner.shutdown().await;
    result
}
