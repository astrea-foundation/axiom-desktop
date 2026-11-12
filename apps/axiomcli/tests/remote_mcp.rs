use axiom_acp_extension::{McpAuthentication, McpRemoteConnection};
use axiomcli::{
    app::{PermissionProfile, SessionId},
    config::McpServerConfig,
    mcp::connect_tools,
    tools::ToolContext,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "fixture-http-private-token-928371";
#[derive(Clone)]
struct HttpTools {
    modern: bool,
    calls: Arc<AtomicUsize>,
    slow: bool,
}
impl ServerHandler for HttpTools {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        if self.modern {
            Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
        } else {
            Cow::Borrowed(&[ProtocolVersion::V_2025_11_25])
        }
    }
    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::DiscoverResult, ErrorData> {
        if self.modern {
            Ok(rmcp::model::DiscoverResult::new(
                vec![ProtocolVersion::V_2026_07_28],
                ServerCapabilities::builder().enable_tools().build(),
            ))
        } else {
            Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "Method not found",
                None,
            ))
        }
    }
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        if !self.modern {
            info.protocol_version = ProtocolVersion::V_2025_11_25;
        }
        info
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![Tool::new(
                "echo",
                "An HTTP fixture",
                Arc::new(
                    json!({"type":"object","properties":{"value":{"type":"string"}}})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.slow {
            context.ct.cancelled().await;
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "{} {TOKEN}",
            request
                .arguments
                .unwrap_or_default()
                .get("value")
                .unwrap_or(&Value::Null)
        ))])
        .into())
    }
}
struct Fixture {
    refreshes: Arc<AtomicUsize>,
    url: String,
    calls: Arc<AtomicUsize>,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}
async fn fixture(modern: bool, json_response: bool, bearer: bool, fault: &'static str) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let url = format!("{base}/mcp");
    let resource = url.clone();
    let issuer = format!("{base}/oauth");
    let resource_metadata = json!({"resource":resource,"authorization_servers":[issuer.clone()],"scopes_supported":["read"]});
    let oauth_metadata = json!({"issuer":issuer,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"registration_endpoint":format!("{base}/register"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"]});
    let refreshes = Arc::new(AtomicUsize::new(0));
    let refresh_count = refreshes.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let tools = HttpTools {
        modern,
        calls: calls.clone(),
        slow: fault == "slow",
    };
    let service: StreamableHttpService<HttpTools, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::default(),
        StreamableHttpServerConfig::default()
            .with_json_response(json_response)
            .with_legacy_session_mode(!modern),
    );
    let observed = calls.clone();
    let metadata = oauth_metadata.clone();
    let protected = resource_metadata.clone();
    let router = axum::Router::new()
        .route("/.well-known/oauth-protected-resource/mcp", axum::routing::get(move || {let value=resource_metadata.clone(); async move {axum::Json(value)}}))
        .route("/.well-known/oauth-protected-resource", axum::routing::get(move || {let value=protected.clone(); async move {axum::Json(value)}}))
        .route("/.well-known/oauth-authorization-server/oauth", axum::routing::get(move || {let value=oauth_metadata.clone(); async move {axum::Json(value)}}))
        .route("/oauth/.well-known/openid-configuration", axum::routing::get(move || {let value=metadata.clone(); async move {axum::Json(value)}}))
        .route("/token", axum::routing::post(move |axum::Form(form): axum::Form<BTreeMap<String,String>>| {
            let count = refresh_count.clone(); let resource=resource.clone(); async move {
                assert_eq!(form.get("grant_type").map(String::as_str),Some("refresh_token"));
                assert_eq!(form.get("refresh_token").map(String::as_str),Some("fixture-refresh"));
                assert_eq!(form.get("resource"),Some(&resource));
                count.fetch_add(1,Ordering::SeqCst);
                axum::Json(json!({"access_token":TOKEN,"token_type":"Bearer","refresh_token":"fixture-rotated-refresh","expires_in":3600}))
            }
        }))
        .nest_service("/mcp",service).route("/redirect",axum::routing::post(|| async { (StatusCode::TEMPORARY_REDIRECT,[("location","/mcp")]) }))
        .layer(axum::middleware::from_fn(move |request: Request<Body>, next: Next| {
            let calls = observed.clone();
            async move {
                if bearer && ["/mcp", "/redirect"].contains(&request.uri().path()) && request.headers().get("authorization").and_then(|h|h.to_str().ok()) != Some(&format!("Bearer {TOKEN}")) {
                    return (StatusCode::UNAUTHORIZED,[("www-authenticate","Bearer")]).into_response();
                }
                if ["expired", "oversize"].contains(&fault) {
                    let (parts,body) = request.into_parts();
                    let bytes = axum::body::to_bytes(body,1024*1024).await.unwrap();
                    let message: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    if message["method"] == "tools/call" {
                        calls.fetch_add(1,Ordering::SeqCst);
                        if fault == "expired" { return (StatusCode::NOT_FOUND, "session expired").into_response(); }
                        return Response::builder().header("content-type","application/json").body(Body::from("x".repeat(1024*1024+1))).unwrap();
                    }
                    next.run(Request::from_parts(parts, Body::from(bytes))).await
                } else { next.run(request).await }
            }
        }));
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    Fixture {
        refreshes,
        url,
        calls,
        cancellation,
        task,
    }
}
fn config(url: String, bearer: bool) -> McpServerConfig {
    McpServerConfig {
        display_name: None,
        name: "remote".into(),
        command: String::new(),
        args: vec![],
        env: if bearer {
            BTreeMap::from([("bearer_token".into(), TOKEN.into())])
        } else {
            BTreeMap::new()
        },
        remote: Some(McpRemoteConnection {
            url,
            auth: if bearer {
                McpAuthentication::Bearer
            } else {
                McpAuthentication::None
            },
            client_id: None,
            client_metadata_url: None,
        }),
        oauth_credentials: None,
        oauth_identity: None,
        read_only_tools: vec![],
        tool_timeout_secs: 1,
    }
}
fn context(temp: &tempfile::TempDir) -> ToolContext {
    ToolContext {
        session_id: SessionId::new(),
        model_id: "fixture".into(),
        cwd: temp.path().to_owned(),
        permission_profile: PermissionProfile::Confirm,
        images: axiomcli::image_tool::ImageCatalog::default(),
        progress: None,
    }
}
#[tokio::test]
async fn remote_json_and_sse_support_modern_and_legacy_and_redact_tokens() {
    for modern in [true, false] {
        for json_response in [true, false] {
            let server = fixture(modern, json_response, true, "").await;
            let tools = connect_tools(&[config(server.url.clone(), true)], 4096)
                .await
                .unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(
                tools[0].mcp_source().unwrap().destination,
                Some(server.url.clone())
            );
            assert_eq!(
                server.calls.load(Ordering::SeqCst),
                0,
                "discovery never calls a tool"
            );
            let temp = tempfile::tempdir().unwrap();
            let result = tools[0]
                .execute(
                    &context(&temp),
                    json!({"value":"hello"}),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert!(result.success);
            assert!(result.content.contains("hello"));
            assert!(!result.content.contains(TOKEN));
            assert_eq!(server.calls.load(Ordering::SeqCst), 1);
            tools[0].shutdown().await;
        }
    }
}
#[tokio::test]
async fn public_remote_works_and_authentication_is_not_inherited() {
    let public = fixture(true, true, false, "").await;
    let tools = connect_tools(&[config(public.url.clone(), false)], 4096)
        .await
        .unwrap();
    tools[0].shutdown().await;
    let private = fixture(true, true, true, "").await;
    assert!(
        connect_tools(&[config(private.url.clone(), false)], 4096)
            .await
            .is_err()
    );
    assert_eq!(private.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn expired_session_and_oversize_response_never_replay_side_effects() {
    for fault in ["expired", "oversize"] {
        let server = fixture(false, true, false, fault).await;
        let tools = connect_tools(&[config(server.url.clone(), false)], 4096)
            .await
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        assert!(
            tools[0]
                .execute(
                    &context(&temp),
                    json!({"value":"one action"}),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(
            server.calls.load(Ordering::SeqCst),
            1,
            "an uncertain remote action must not replay"
        );
        tools[0].shutdown().await;
    }
}
#[tokio::test]
async fn stop_and_deadline_do_not_retry_remote_actions() {
    for stop in [true, false] {
        let server = fixture(true, false, false, "slow").await;
        let tools = connect_tools(&[config(server.url.clone(), false)], 4096)
            .await
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let cancellation = CancellationToken::new();
        let context = context(&temp);
        let execute = tools[0].execute(
            &context,
            json!({"value":"one action"}),
            cancellation.clone(),
        );
        tokio::pin!(execute);
        if stop {
            tokio::select! { result = &mut execute => panic!("finished before Stop: {result:?}"), () = tokio::time::sleep(Duration::from_millis(100)) => {} }
            cancellation.cancel();
        }
        assert!(execute.await.is_err());
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        tools[0].shutdown().await;
    }
}
#[tokio::test]
async fn remote_transport_does_not_follow_redirects() {
    let server = fixture(true, true, true, "").await;
    let url = server.url.replace("/mcp", "/redirect");
    assert!(connect_tools(&[config(url, true)], 4096).await.is_err());
    assert_eq!(server.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oauth_refresh_is_shared_across_turns_and_issuer_changes_fail_closed() {
    use rmcp::transport::auth::{CredentialStore, InMemoryCredentialStore, StoredCredentials};
    let server = fixture(true, true, true, "").await;
    let store = InMemoryCredentialStore::new();
    let issuer = format!("{}/oauth", server.url.trim_end_matches("/mcp"));
    let credentials: StoredCredentials = serde_json::from_value(json!({
        "client_id":"fixture-client", "issuer":issuer,
        "token_response":{"access_token":"expired-private-token","token_type":"Bearer","refresh_token":"fixture-refresh","expires_in":1,"scope":"read"},
        "granted_scopes":["read"],"token_received_at":1,
    })).unwrap();
    store.save(credentials).await.unwrap();
    let mut config = config(server.url.clone(), false);
    config.remote.as_mut().unwrap().auth = McpAuthentication::Oauth;
    config.oauth_credentials = Some(Arc::new(store.clone()));
    config.oauth_identity = Some(uuid::Uuid::new_v4().to_string());
    let first_config = [config.clone()];
    let second_config = [config.clone()];
    let (first, second) = tokio::join!(
        connect_tools(&first_config, 4096),
        connect_tools(&second_config, 4096)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(
        store.load().await.unwrap().unwrap().granted_scopes,
        vec!["read"]
    );
    assert_eq!(
        server.refreshes.load(Ordering::SeqCst),
        1,
        "rotating refresh tokens must have one writer across turns"
    );
    let temp = tempfile::tempdir().unwrap();
    let result = first[0]
        .execute(
            &context(&temp),
            json!({"value":"OAuth result"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.content.contains(TOKEN));
    assert!(result.content.contains("OAuth result"));
    first[0].shutdown().await;
    second[0].shutdown().await;
    let mut changed = store.load().await.unwrap().unwrap();
    changed.issuer = Some("https://different-issuer.example".into());
    store.save(changed).await.unwrap();
    config.oauth_identity = Some(uuid::Uuid::new_v4().to_string());
    assert!(connect_tools(&[config], 4096).await.is_err());
}
