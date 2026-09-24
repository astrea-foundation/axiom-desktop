#![cfg(all(feature = "test-fixture", target_os = "linux"))]

use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axiom_secure_client::test_support::{
    FixtureModelIdentity, encrypt_for_client, sign_attestation, sign_response_receipt,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

const MODEL_ID: &str = "fixture-model";
const UPSTREAM_MODEL: &str = "fixture/provider-model";
const LOCAL_TOKEN: &str = "local-test-token-0123456789-abcdef";
const UPSTREAM_KEY: &str = "fixture-upstream-key-must-never-be-logged";

struct FixtureState {
    model_identity: FixtureModelIdentity,
    provider_base_url: String,
    relay_bodies: Mutex<Vec<String>>,
    decrypted_values: Mutex<Vec<Vec<String>>>,
    slow_stream_started: Notify,
    slow_stream_dropped: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct AttestationQuery {
    model: String,
    nonce: String,
}

struct ProxyProcess {
    child: Child,
    base_url: String,
    stderr_reader: std::thread::JoinHandle<String>,
}

impl ProxyProcess {
    fn start(relay_url: &str) -> Self {
        Self::start_with(relay_url, &[])
    }

    fn start_with(relay_url: &str, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_axiom-proxy"))
            .args(["--bind", "127.0.0.1:0", "--axiom-base-url", relay_url])
            .args(extra)
            .env("AXIOM_API_KEY", UPSTREAM_KEY)
            .env("AXIOM_PROXY_TOKEN", LOCAL_TOKEN)
            .env("AXIOM_TEST_FIXTURE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("actual proxy binary starts");
        let stdout = child.stdout.take().expect("proxy stdout is piped");
        let mut stderr = child.stderr.take().expect("proxy stderr is piped");
        let stderr_reader = std::thread::spawn(move || {
            let mut output = String::new();
            stderr
                .read_to_string(&mut output)
                .expect("read proxy stderr");
            output
        });
        let mut stdout = BufReader::new(stdout);
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read ready line");
        let ready: Value = serde_json::from_str(&line).expect("ready line is JSON");
        let address = ready["address"].as_str().expect("ready address");
        Self {
            child,
            base_url: format!("http://{address}"),
            stderr_reader,
        }
    }

    fn stop(mut self) -> String {
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().expect("poll proxy") {
                assert!(
                    status.success(),
                    "proxy did not shut down cleanly: {status}"
                );
                break;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("proxy did not stop after SIGINT");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.stderr_reader.join().expect("join stderr reader")
    }

    fn id(&self) -> u32 {
        self.child.id()
    }
}

#[derive(Debug)]
struct LinuxResources {
    file_descriptors: usize,
    resident_kib: u64,
}

fn linux_resources(process_id: u32) -> LinuxResources {
    let file_descriptors = std::fs::read_dir(format!("/proc/{process_id}/fd"))
        .expect("read proxy file descriptors")
        .count();
    let status = std::fs::read_to_string(format!("/proc/{process_id}/status"))
        .expect("read proxy process status");
    let resident_kib = status
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
        .expect("proxy VmRSS is present");
    LinuxResources {
        file_descriptors,
        resident_kib,
    }
}

/// A request from an application the operator cannot modify must still be
/// answered, and the parameters that were not applied must be discoverable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_proxy_honors_compatibility_mode_for_unsupported_parameters() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_address = listener.local_addr().unwrap();
    let relay_url = format!("http://{fixture_address}");
    let state = Arc::new(FixtureState {
        model_identity: FixtureModelIdentity::generate(),
        provider_base_url: format!("{relay_url}/v1"),
        relay_bodies: Mutex::new(Vec::new()),
        decrypted_values: Mutex::new(Vec::new()),
        slow_stream_started: Notify::new(),
        slow_stream_dropped: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/api/v1/relay/models", get(catalog))
        .route("/v1/attestation/report", get(attestation))
        .route("/api/v1/relay/chat/completions", post(relay_chat))
        .with_state(Arc::clone(&state));
    let fixture_shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown = fixture_shutdown.clone();
    let fixture_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder().build().unwrap();

    // A body shaped like what a third-party application actually sends: an
    // OpenAI parameter the relay has no field for, an inert one, a typo, an
    // `n` that is a no-op, and text delivered as a part array.
    let body = json!({
        "model":MODEL_ID,
        "messages":[{"role":"user","content":[{"type":"text","text":"compat fixture request"}]}],
        "stop":["\n\n"],
        "seed":11,
        "user":"local-app",
        "temperatur":0.3,
        "n":1
    });

    let lenient = ProxyProcess::start_with(&relay_url, &["--compat", "lenient"]);
    let answered = post_json(&client, &lenient.base_url, body.clone()).await;
    assert_eq!(answered.status(), StatusCode::OK);
    let reported = answered
        .headers()
        .get("x-axiom-ignored-parameters")
        .expect("lenient mode reports dropped parameters")
        .to_str()
        .unwrap()
        .to_owned();
    // Answer-affecting parameters and the typo are named; the inert one is not.
    assert_eq!(reported, "seed,stop,temperatur");
    let answer: Value = answered.json().await.unwrap();
    assert_eq!(answer["choices"][0]["message"]["role"], "assistant");

    // The part array became text, and nothing unsupported reached the relay.
    let relayed = state.relay_bodies.lock().unwrap().last().cloned().unwrap();
    for absent in ["stop", "seed", "temperatur", "local-app"] {
        assert!(
            !relayed.contains(absent),
            "dropped parameter {absent} must never reach the relay"
        );
    }

    let lenient_stderr = lenient.stop();
    assert!(
        lenient_stderr.contains("request_parameters_ignored"),
        "the supervisor log must record the drop"
    );

    let strict = ProxyProcess::start_with(&relay_url, &["--compat", "strict"]);
    let refused = post_json(&client, &strict.base_url, body).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let named = refused
        .headers()
        .get("x-axiom-ignored-parameters")
        .expect("strict mode names the refused parameters")
        .to_str()
        .unwrap()
        .to_owned();
    // Strict promises full fidelity, so it names the inert parameter as well.
    assert!(named.contains("user"), "strict named: {named}");
    assert!(named.contains("stop"), "strict named: {named}");

    // A request carrying nothing unsupported still succeeds under strict mode.
    let clean = post_json(
        &client,
        &strict.base_url,
        json!({
            "model":MODEL_ID,
            "messages":[{"role":"user","content":"compat fixture request"}]
        }),
    )
    .await;
    assert_eq!(clean.status(), StatusCode::OK);
    assert!(clean.headers().get("x-axiom-ignored-parameters").is_none());

    // More choices than can be produced is refused regardless of mode.
    let over_delivered = post_json(
        &client,
        &strict.base_url,
        json!({
            "model":MODEL_ID,
            "messages":[{"role":"user","content":"compat fixture request"}],
            "n":2
        }),
    )
    .await;
    assert_eq!(over_delivered.status(), StatusCode::BAD_REQUEST);

    strict.stop();
    fixture_shutdown.cancel();
    fixture_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_proxy_preserves_encryption_streaming_tools_errors_and_cancellation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_address = listener.local_addr().unwrap();
    let relay_url = format!("http://{fixture_address}");
    let state = Arc::new(FixtureState {
        model_identity: FixtureModelIdentity::generate(),
        provider_base_url: format!("{relay_url}/v1"),
        relay_bodies: Mutex::new(Vec::new()),
        decrypted_values: Mutex::new(Vec::new()),
        slow_stream_started: Notify::new(),
        slow_stream_dropped: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/api/v1/relay/models", get(catalog))
        .route("/v1/attestation/report", get(attestation))
        .route("/api/v1/relay/chat/completions", post(relay_chat))
        .with_state(Arc::clone(&state));
    let fixture_shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown = fixture_shutdown.clone();
    let fixture_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });

    let proxy = ProxyProcess::start(&relay_url);
    let client = reqwest::Client::builder().build().unwrap();

    let unauthorized = client
        .get(format!("{}/v1/models", proxy.base_url))
        .bearer_auth("wrong-local-token")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert!(state.relay_bodies.lock().unwrap().is_empty());

    let models: Value = authorized(&client, &format!("{}/v1/models", proxy.base_url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], MODEL_ID);

    let privacy_sentinels = [
        "PRIVATE-system-7b9c",
        "PRIVATE-name-7b9c",
        "PRIVATE-user-7b9c",
        "PRIVATE-reasoning-7b9c",
        "PRIVATE-refusal-7b9c",
        "PRIVATE-history-tool-7b9c",
        "PRIVATE-history-args-7b9c",
        "PRIVATE-tool-result-7b9c",
        "PRIVATE-tool-name-7b9c",
        "PRIVATE-tool-description-7b9c",
        "PRIVATE-schema-7b9c",
    ];
    let privacy_request = json!({
        "model": MODEL_ID,
        "messages": [
            {"role":"system","content":privacy_sentinels[0],"name":privacy_sentinels[1]},
            {"role":"user","content":privacy_sentinels[2]},
            {"role":"assistant","content":"history","reasoning_content":privacy_sentinels[3],
             "refusal":privacy_sentinels[4],"tool_calls":[{"id":"call-history","type":"function",
             "function":{"name":privacy_sentinels[5],"arguments":json!({"value":privacy_sentinels[6]}).to_string()}}]},
            {"role":"tool","tool_call_id":"call-history","content":privacy_sentinels[7]}
        ],
        "tools":[{"type":"function","function":{"name":privacy_sentinels[8],
            "description":privacy_sentinels[9],
            "parameters":{"type":"object","properties":{"secret":{"const":privacy_sentinels[10]}}}}}],
        "tool_choice":{"type":"function","function":{"name":privacy_sentinels[8]}},
        "reasoning_effort":"medium"
    });
    let privacy_response = post_json(&client, &proxy.base_url, privacy_request).await;
    assert_eq!(privacy_response.status(), StatusCode::OK);
    let privacy_response: Value = privacy_response.json().await.unwrap();
    assert_eq!(
        privacy_response["choices"][0]["message"]["content"],
        "fixture answer"
    );

    let tool_request = json!({
        "model": MODEL_ID,
        "messages":[{"role":"user","content":"tool fixture request"}],
        "tools":[{"type":"function","function":{"name":"read_file","description":"read one file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}],
        "tool_choice":"required",
        "reasoning_effort":"medium"
    });
    let tool_response: Value = post_json(&client, &proxy.base_url, tool_request)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tool_response["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        tool_response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    assert_eq!(
        tool_response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"README.md\"}"
    );

    let follow_up = json!({
        "model": MODEL_ID,
        "messages":[
            {"role":"user","content":"tool fixture request"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"call-fixture-1","type":"function",
                "function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}}]},
            {"role":"tool","tool_call_id":"call-fixture-1","content":"secret tool result 55fa"}
        ],
        "tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}],
        "reasoning_effort":"medium"
    });
    let follow_up: Value = post_json(&client, &proxy.base_url, follow_up)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        follow_up["choices"][0]["message"]["content"],
        "tool result accepted"
    );

    let parallel: Value = post_json(
        &client,
        &proxy.base_url,
        json!({
            "model":MODEL_ID,
            "messages":[{"role":"user","content":"parallel tool fixture request"}],
            "tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}],
            "tool_choice":"auto","parallel_tool_calls":true,"reasoning_effort":"medium"
        }),
    )
    .await
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        parallel["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let stream_body = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"stream fixture request"}],
            "stream":true,"stream_options":{"include_usage":true},"reasoning_effort":"medium"}),
    )
    .await
    .error_for_status()
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(stream_body.contains("fixture streamed answer"));
    assert!(stream_body.contains("fixture reasoning"));
    assert!(stream_body.contains("\"usage\""));
    assert!(stream_body.ends_with("data: [DONE]\n\n"));

    let stream_tool_body = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"stream tool fixture request"}],
            "tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}],
            "stream":true,"reasoning_effort":"medium"}),
    )
    .await
    .error_for_status()
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(stream_tool_body.contains("read_"));
    assert!(stream_tool_body.contains("file"));
    assert!(stream_tool_body.contains("README.md"));
    assert!(stream_tool_body.contains("tool_calls"));

    let credit = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"credit error fixture"}]}),
    )
    .await;
    assert_eq!(credit.status(), StatusCode::PAYMENT_REQUIRED);
    let credit: Value = credit.json().await.unwrap();
    assert_eq!(credit["error"]["code"], "insufficient_credit");

    let tampered = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"tamper response fixture"}]}),
    )
    .await;
    assert_eq!(tampered.status(), StatusCode::BAD_GATEWAY);
    let tampered: Value = tampered.json().await.unwrap();
    assert_eq!(tampered["error"]["code"], "secure_inference_failed");

    let abrupt = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"abrupt stream fixture"}],"stream":true}),
    )
    .await
    .error_for_status()
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(abrupt.contains("upstream_failed"));
    assert!(!abrupt.contains("data: [DONE]"));
    assert!(!abrupt.contains("\"finish_reason\":\"stop\""));

    let invalid_receipt = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"bad stream receipt fixture"}],"stream":true}),
    )
    .await
    .error_for_status()
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(invalid_receipt.contains("\"error\""), "{invalid_receipt}");
    assert!(!invalid_receipt.contains("data: [DONE]"));
    assert!(!invalid_receipt.contains("\"finish_reason\":\"stop\""));

    let mut slow = post_json(
        &client,
        &proxy.base_url,
        json!({"model":MODEL_ID,"messages":[{"role":"user","content":"slow stream fixture"}],"stream":true}),
    )
    .await
    .error_for_status()
    .unwrap();
    state.slow_stream_started.notified().await;
    let _ = slow.chunk().await.unwrap();
    drop(slow);
    wait_for_flag(&state.slow_stream_dropped).await;

    let parallel_requests = (0..8).map(|index| {
        let client = client.clone();
        let base = proxy.base_url.clone();
        tokio::spawn(async move {
            let response: Value = post_json(
                &client,
                &base,
                json!({"model":MODEL_ID,"messages":[{"role":"user","content":format!("parallel session {index}")}]}),
            )
            .await
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
            assert_eq!(response["choices"][0]["message"]["content"], "fixture answer");
        })
    });
    for task in parallel_requests {
        task.await.unwrap();
    }

    let resources_before_soak = linux_resources(proxy.id());
    for index in 0..24 {
        let response = post_json(
            &client,
            &proxy.base_url,
            json!({"model":MODEL_ID,"messages":[{"role":"user","content":format!("stream soak {index}")}],"stream":true}),
        )
        .await
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
        assert!(response.ends_with("data: [DONE]\n\n"));
    }
    for _ in 0..12 {
        state.slow_stream_dropped.store(false, Ordering::SeqCst);
        let mut response = post_json(
            &client,
            &proxy.base_url,
            json!({"model":MODEL_ID,"messages":[{"role":"user","content":"slow stream fixture"}],"stream":true}),
        )
        .await
        .error_for_status()
        .unwrap();
        state.slow_stream_started.notified().await;
        let _ = response.chunk().await.unwrap();
        drop(response);
        wait_for_flag(&state.slow_stream_dropped).await;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    let resources_after_soak = linux_resources(proxy.id());
    assert!(
        resources_after_soak.file_descriptors <= resources_before_soak.file_descriptors + 24,
        "proxy file descriptors did not return to a bounded level: before={resources_before_soak:?}, after={resources_after_soak:?}"
    );
    assert!(
        resources_after_soak.resident_kib <= resources_before_soak.resident_kib + 96 * 1024,
        "proxy resident memory grew beyond the soak bound: before={resources_before_soak:?}, after={resources_after_soak:?}"
    );

    let relay_bodies = state.relay_bodies.lock().unwrap().clone();
    assert!(relay_bodies.len() >= 17);
    for body in &relay_bodies {
        for sentinel in privacy_sentinels
            .iter()
            .chain(["read_file", "README.md", "secret tool result 55fa"].iter())
        {
            assert!(
                !body.contains(sentinel),
                "plaintext leaked into relay JSON: {sentinel}"
            );
        }
    }
    let decrypted: BTreeSet<String> = state
        .decrypted_values
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .cloned()
        .collect();
    for sentinel in privacy_sentinels {
        assert!(
            decrypted.iter().any(|value| value.contains(sentinel)),
            "fixture did not recover {sentinel}"
        );
    }

    let proxy_logs = proxy.stop();
    assert!(proxy_logs.contains("\"event\":\"security_verified\""));
    assert!(proxy_logs.contains("\"terminal\":\"cancelled\""));
    for secret in [UPSTREAM_KEY, LOCAL_TOKEN]
        .into_iter()
        .chain(privacy_sentinels)
    {
        assert!(
            !proxy_logs.contains(secret),
            "secret leaked into proxy logs: {secret}"
        );
    }
    fixture_shutdown.cancel();
    fixture_task.await.unwrap();
}

async fn catalog(State(state): State<Arc<FixtureState>>) -> Json<Value> {
    Json(json!([{
        "id":MODEL_ID,
        "label":"Encrypted Fixture Model",
        "short_label":"Fixture",
        "model":UPSTREAM_MODEL,
        "base_url":state.provider_base_url,
        "provider":"axiom-test-fixture",
        "provider_label":"Axiom Test Fixture",
        "e2ee_protocol":"near-v3",
        "e2ee_encryption_version":2,
        "attestation_protocol":"signed-test-fixture-v1",
        "relay_contract_version":2,
        "context_window_tokens":131_072,
        "max_output_tokens":8192,
        "supported_reasoning_efforts":["medium","high"]
    }]))
}

async fn attestation(
    State(state): State<Arc<FixtureState>>,
    Query(query): Query<AttestationQuery>,
) -> Json<Value> {
    let model_public_key_hex = state.model_identity.public_key_hex();
    let signature_hex = sign_attestation(
        &query.model,
        &query.nonce,
        &state.provider_base_url,
        &model_public_key_hex,
    );
    Json(json!({
        "model":query.model,
        "nonce_hex":query.nonce,
        "model_public_key_hex":model_public_key_hex,
        "signature_hex":signature_hex
    }))
}

async fn relay_chat(State(state): State<Arc<FixtureState>>, body: Bytes) -> Response {
    let raw = String::from_utf8(body.to_vec()).unwrap();
    state.relay_bodies.lock().unwrap().push(raw.clone());
    let request: Value = serde_json::from_str(&raw).unwrap();
    let mut decrypted = Vec::new();
    decrypt_encrypted_values(&state.model_identity, &request, &mut decrypted);
    state
        .decrypted_values
        .lock()
        .unwrap()
        .push(decrypted.clone());
    let joined = decrypted.join("\n");
    if joined.contains("credit error fixture") {
        return (
            StatusCode::PAYMENT_REQUIRED,
            "fixture credit body must not be reflected",
        )
            .into_response();
    }
    let client_key = request["client_public_key_hex"].as_str().unwrap();
    let request_hash = request["provider_e2ee_context"]["request_body_hash"]
        .as_str()
        .unwrap();
    if request["stream"].as_bool() == Some(true) {
        return stream_response(&state, client_key, request_hash, &joined);
    }
    let response = if joined.contains("tamper response fixture") {
        json!({
            "id":"fixture-tampered","model":UPSTREAM_MODEL,
            "encrypted_content":"00".repeat(72),"finish_reason":"stop","usage":{}
        })
    } else if joined.contains("parallel tool fixture request") {
        json!({
            "id":"fixture-parallel","model":UPSTREAM_MODEL,"encrypted_tool_calls":[
                encrypted_tool_call(client_key,"call-fixture-a","read_file","{\"path\":\"A.md\"}"),
                encrypted_tool_call(client_key,"call-fixture-b","read_file","{\"path\":\"B.md\"}")
            ],"finish_reason":"tool_calls","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}
        })
    } else if joined.contains("tool fixture request") && !joined.contains("secret tool result 55fa")
    {
        json!({
            "id":"fixture-tool","model":UPSTREAM_MODEL,
            "encrypted_tool_calls":[encrypted_tool_call(client_key,"call-fixture-1","read_file","{\"path\":\"README.md\"}")],
            "finish_reason":"tool_calls","usage":{"prompt_tokens":8,"completion_tokens":4,"total_tokens":12}
        })
    } else {
        let answer = if joined.contains("secret tool result 55fa") {
            "tool result accepted"
        } else {
            "fixture answer"
        };
        json!({
            "id":"fixture-complete","model":UPSTREAM_MODEL,
            "encrypted_content":encrypt_for_client(client_key,answer.as_bytes()).unwrap(),
            "finish_reason":"stop","usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}
        })
    };
    let proof = completion_proof(&response, request_hash);
    let mut response = response;
    response["proof"] = proof;
    Json(response).into_response()
}

fn completion_proof(response: &Value, request_hash: &str) -> Value {
    let mut message = serde_json::Map::new();
    if let Some(content) = response.get("encrypted_content") {
        message.insert("content".into(), content.clone());
    }
    if let Some(reasoning) = response.get("encrypted_reasoning_content") {
        message.insert("reasoning_content".into(), reasoning.clone());
    }
    if let Some(refusal) = response.get("encrypted_refusal") {
        message.insert("refusal".into(), refusal.clone());
    }
    if let Some(calls) = response
        .get("encrypted_tool_calls")
        .and_then(Value::as_array)
    {
        message.insert(
            "tool_calls".into(),
            Value::Array(
                calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call["id"],
                            "type": call["type"],
                            "function": {
                                "name": call["function"]["encrypted_name"],
                                "arguments": call["function"]["encrypted_arguments"],
                            },
                        })
                    })
                    .collect(),
            ),
        );
    }
    let raw = json!({
        "id": response["id"],
        "model": response["model"],
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": response["finish_reason"],
        }],
        "usage": response["usage"],
    });
    let bytes = serde_json::to_vec(&raw).unwrap();
    sign_response_receipt(
        &bytes,
        request_hash,
        response["id"].as_str().unwrap(),
        UPSTREAM_MODEL,
    )
}

fn stream_response(
    state: &Arc<FixtureState>,
    client_key: &str,
    request_hash: &str,
    joined: &str,
) -> Response {
    let run_id = "fixture-stream";
    let mut frames = vec![sse(
        "run.created",
        &json!({"run_id":run_id,"inference_encryption":"provider_e2ee_v2"}),
    )];
    if joined.contains("abrupt stream fixture") {
        return sse_body(frames);
    }
    if joined.contains("slow stream fixture") {
        state.slow_stream_started.notify_one();
        let dropped = Arc::clone(&state.slow_stream_dropped);
        let key = client_key.to_owned();
        let stream = stream::unfold(
            (0_u64, DropSignal(dropped), key),
            |(sequence, guard, key)| async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                let frame = if sequence == 0 {
                    sse(
                        "run.created",
                        &json!({"run_id":"fixture-slow","inference_encryption":"provider_e2ee_v2"}),
                    )
                } else {
                    sse(
                        "message.encrypted_delta",
                        &json!({"run_id":"fixture-slow","sequence":sequence,
                    "encrypted_delta":encrypt_for_client(&key,b"x").unwrap()}),
                    )
                };
                Some((
                    Ok::<Bytes, std::convert::Infallible>(Bytes::from(frame)),
                    (sequence + 1, guard, key),
                ))
            },
        );
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap();
    }
    let mut raw_chunks = Vec::new();
    let (usage, finish_reason) = if joined.contains("stream tool fixture request") {
        for (sequence, (name, arguments, id)) in [
            (Some("read_"), Some("{\"path\":"), Some("call-stream-1")),
            (Some("file"), Some("\"README.md\"}"), None),
        ]
        .into_iter()
        .enumerate()
        {
            let encrypted_name = encrypt_for_client(client_key, name.unwrap().as_bytes()).unwrap();
            let encrypted_arguments =
                encrypt_for_client(client_key, arguments.unwrap().as_bytes()).unwrap();
            frames.push(sse(
                "message.encrypted_delta",
                &json!({
                    "run_id":run_id,"sequence":sequence + 1,
                    "encrypted_tool_calls":[{
                        "index":0,"id":id,"type":id.map(|_| "function"),
                        "function":{
                            "encrypted_name":encrypted_name,
                            "encrypted_arguments":encrypted_arguments,
                        }
                    }]
                }),
            ));
            raw_chunks.push(json!({
                "id": run_id,
                "model": UPSTREAM_MODEL,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": id,
                            "type": id.map(|_| "function"),
                            "function": {
                                "name": encrypted_name,
                                "arguments": encrypted_arguments,
                            },
                        }],
                    },
                }],
            }));
        }
        (
            json!({"prompt_tokens":9,"completion_tokens":4,"total_tokens":13}),
            "tool_calls",
        )
    } else {
        let encrypted_reasoning = encrypt_for_client(client_key, b"fixture reasoning").unwrap();
        frames.push(sse(
            "message.encrypted_delta",
            &json!({"run_id":run_id,"sequence":1,
                "encrypted_reasoning_delta":encrypted_reasoning}),
        ));
        raw_chunks.push(json!({
            "id": run_id,
            "model": UPSTREAM_MODEL,
            "choices": [{"index": 0, "delta": {"reasoning_content": encrypted_reasoning}}],
        }));
        let encrypted_content = encrypt_for_client(client_key, b"fixture streamed answer").unwrap();
        frames.push(sse(
            "message.encrypted_delta",
            &json!({"run_id":run_id,"sequence":2,
                "encrypted_delta":encrypted_content}),
        ));
        raw_chunks.push(json!({
            "id": run_id,
            "model": UPSTREAM_MODEL,
            "choices": [{"index": 0, "delta": {"content": encrypted_content}}],
        }));
        (
            json!({"prompt_tokens":11,"completion_tokens":6,"total_tokens":17}),
            "stop",
        )
    };
    raw_chunks.push(json!({
        "id": run_id,
        "model": UPSTREAM_MODEL,
        "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
        "usage": usage,
    }));
    let mut raw_body = String::new();
    for chunk in &raw_chunks {
        raw_body.push_str("data: ");
        raw_body.push_str(&chunk.to_string());
        raw_body.push_str("\n\n");
    }
    raw_body.push_str("data: [DONE]\n\n");
    let mut proof =
        sign_response_receipt(raw_body.as_bytes(), request_hash, run_id, UPSTREAM_MODEL);
    if joined.contains("bad stream receipt fixture") {
        proof["signature"] = json!(format!("0x{}", "00".repeat(65)));
    }
    frames.push(sse(
        "message.encrypted_completed",
        &json!({"run_id":run_id,"usage":usage,"finish_reason":finish_reason,"proof":proof}),
    ));
    frames.push(sse("run.completed", &json!({"run_id":run_id})));
    sse_body(frames)
}

struct DropSignal(Arc<AtomicBool>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn sse_body(frames: Vec<String>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(frames.into_iter().collect::<String>()))
        .unwrap()
}

fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn encrypted_tool_call(client_key: &str, id: &str, name: &str, arguments: &str) -> Value {
    json!({
        "id":id,"type":"function","function":{
            "encrypted_name":encrypt_for_client(client_key,name.as_bytes()).unwrap(),
            "encrypted_arguments":encrypt_for_client(client_key,arguments.as_bytes()).unwrap()
        }
    })
}

fn decrypt_encrypted_values(
    identity: &FixtureModelIdentity,
    value: &Value,
    output: &mut Vec<String>,
) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "encrypted_content"
                        | "encrypted_reasoning_content"
                        | "encrypted_refusal"
                        | "encrypted_name"
                        | "encrypted_description"
                        | "encrypted_parameters"
                        | "encrypted_arguments"
                        | "encrypted_delta"
                        | "encrypted_reasoning_delta"
                        | "encrypted_refusal_delta"
                ) && let Some(ciphertext) = value.as_str()
                {
                    let bytes = identity.decrypt_hex(ciphertext).unwrap();
                    output.push(String::from_utf8(bytes).unwrap());
                    continue;
                }
                decrypt_encrypted_values(identity, value, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                decrypt_encrypted_values(identity, value, output);
            }
        }
        _ => {}
    }
}

fn authorized(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    client.get(url).bearer_auth(LOCAL_TOKEN)
}

async fn post_json(client: &reqwest::Client, base_url: &str, body: Value) -> reqwest::Response {
    client
        .post(format!("{base_url}/v1/chat/completions"))
        .bearer_auth(LOCAL_TOKEN)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn wait_for_flag(flag: &AtomicBool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !flag.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "upstream stream was not cancelled"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
