use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::{Body, Bytes};
use http_body_util::BodyExt;
use pengepul::app::{
    ModelsFuture, UpstreamClient, UpstreamJsonResponse, UpstreamRequest, UpstreamSseResponse,
    create_app, create_app_with_upstream,
};
use pengepul::config::{CloakingConfig, Config, DebugMode, TimeoutConfig};
use pengepul::models::FetchedModels;
use pengepul::tokens::save_token;
use pengepul::types::{AvailableAccount, ProviderId, ProviderKind, TokenData};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct FakeUpstream {
    calls: Mutex<Vec<UpstreamRequest>>,
}

impl FakeUpstream {
    fn calls(&self) -> Vec<UpstreamRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[derive(Default)]
struct RetryUpstream {
    calls: Mutex<Vec<UpstreamRequest>>,
}

impl RetryUpstream {
    fn calls(&self) -> Vec<UpstreamRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl UpstreamClient for RetryUpstream {
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("generic chat not used in retry test")
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("generic stream not used in retry test")
    }

    fn anthropic_messages(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let status = if request.account.token.access_token.contains("alice") {
            axum::http::StatusCode::TOO_MANY_REQUESTS
        } else {
            axum::http::StatusCode::OK
        };
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async move {
            Ok(UpstreamJsonResponse {
                status,
                body: if status.is_success() {
                    json!({
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-sonnet-4-6",
                        "content": [{"type": "text", "text": "pong"}],
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    })
                } else {
                    json!({"error": {"message": "rate limited"}})
                },
            })
        })
    }

    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("stream not used in retry test")
    }

    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in retry test")
    }

    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in retry test")
    }

    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in retry test")
    }

    fn fetch_models(
        &self,
        _kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        Box::pin(async { Ok(FetchedModels::new(Vec::new())) })
    }
}

impl UpstreamClient for FakeUpstream {
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("generic chat not used in fake tests")
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("generic stream not used in fake tests")
    }

    fn anthropic_messages(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-6",
                    "content": [{"type": "text", "text": "pong"}],
                    "usage": {"input_tokens": 1, "output_tokens": 1}
                }),
            })
        })
    }

    fn anthropic_messages_stream(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamSseResponse {
                status: axum::http::StatusCode::OK,
                body: Box::pin(futures_util::stream::iter([
                    Ok(Bytes::from_static(
                        b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                    )),
                    Ok(Bytes::from_static(b"event: message_stop\ndata: {}\n\n")),
                ])),
            })
        })
    }

    fn anthropic_count_tokens(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({"input_tokens": 2}),
            })
        })
    }

    fn codex_responses(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "resp_1",
                    "object": "response",
                    "status": "completed",
                    "model": "gpt-5.4",
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "ok"}]
                    }],
                    "output_text": "ok",
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                }),
            })
        })
    }

    fn codex_responses_stream(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamSseResponse {
                status: axum::http::StatusCode::OK,
                body: Box::pin(futures_util::stream::iter([
                    Ok(Bytes::from_static(
                        b"event: response.created\ndata: {\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.4\"}}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"event: response.output_text.delta\ndata: {\"delta\":\"ok\"}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"event: response.completed\ndata: {\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
                    )),
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                ])),
            })
        })
    }

    fn fetch_models(
        &self,
        _kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        Box::pin(async move { Ok(FetchedModels::new(Vec::new())) })
    }
}

fn config(auth_dir: PathBuf) -> Config {
    Config {
        host: String::new(),
        port: 8317,
        auth_dir,
        api_keys: HashSet::from(["sk-test".to_string()]),
        body_limit: "200mb".to_string(),
        cloaking: CloakingConfig {
            cli_version: "2.1.88".to_string(),
            entrypoint: "cli".to_string(),
            codex: BTreeMap::default(),
        },
        timeouts: TimeoutConfig {
            messages_ms: 120_000,
            stream_messages_ms: 600_000,
            count_tokens_ms: 30_000,
        },
        stats_enabled: true,
        debug: DebugMode::Off,
        providers: BTreeMap::default(),
    }
}

async fn json_response(app: axum::Router, request: axum::http::Request<Body>) -> (u16, Value) {
    let response = app.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&body).expect("json body"))
}

async fn raw_response(
    app: axum::Router,
    request: axum::http::Request<Body>,
) -> (u16, axum::http::HeaderMap, String) {
    let response = app.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8(body.to_vec()).expect("utf8 body"),
    )
}

#[tokio::test]
async fn app_auth_and_no_account_responses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = create_app(config(tmp.path().to_path_buf()));

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"status": "ok"}));

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(body["error"]["message"], "missing API key");

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer wrong")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(body["error"]["message"], "invalid API key");

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(body["error"]["type"], "no_account_for_provider");

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages/count_tokens")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"model": "gpt-5.4", "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 501);
    assert_eq!(body["error"]["provider"], "codex");
}

#[tokio::test]
async fn request_without_model_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = create_app(config(tmp.path().to_path_buf()));

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"messages": [{"role": "user", "content": "hi"}]}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400, "missing model must be rejected, not defaulted");
    assert_eq!(body["error"]["message"], "model is required");

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages/count_tokens")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"messages": [{"role": "user", "content": "hi"}]}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"]["message"], "model is required");
}

#[tokio::test]
async fn removed_provider_model_ids_are_rejected_as_unknown() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    // A removed provider's prefixed model id is rejected by the generic unknown-model
    // path and never reaches an upstream.
    for uri in ["/v1/chat/completions", "/v1/messages", "/v1/responses"] {
        let (status, body) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "256")
                .body(Body::from(
                    json!({
                        "model": "opencode/glm-5.1",
                        "messages": [{"role": "user", "content": "hi"}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;

        assert_eq!(status, 400, "{uri}: expected 400, got {status}: {body}");
        assert_eq!(body["error"]["message"], "unknown model: opencode/glm-5.1");
        assert!(
            upstream.calls().is_empty(),
            "{uri}: upstream must not be called"
        );
    }
}

#[tokio::test]
async fn app_models_returns_a_well_formed_list() {
    // The catalog is populated off the request path from live upstream fetches; the fake
    // upstream returns nothing, so this asserts the response contract, not its content.
    // Real catalog content is verified against live accounts.
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .uri("/v1/models")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["object"], "list");
    assert!(body["data"].is_array(), "data must be a list");
}

#[tokio::test]
async fn app_rejects_an_unknown_model() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    // An id no fetched list and no heuristic claims is rejected, not silently routed.
    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"model": "gemini-3", "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 400);
    assert_eq!(body["error"]["message"], "unknown model: gemini-3");
}

#[tokio::test]
async fn app_codex_count_tokens_is_unsupported() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = create_app(config(tmp.path().to_path_buf()));

    // count_tokens is anthropic-only; a codex model answers 501 with the provider name.
    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages/count_tokens")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "256")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "hi"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 501);
    assert_eq!(body["error"]["provider"], "codex");
}

#[tokio::test]
async fn leftover_opencode_credentials_are_ignored_and_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Seed a leftover opencode token exactly as the removed provider wrote it.
    let opencode_dir = tmp.path().join("opencode");
    std::fs::create_dir_all(&opencode_dir).expect("opencode dir");
    std::fs::write(
        opencode_dir.join("opencode-acct.json"),
        serde_json::json!({
            "access_token": "sk-opencode",
            "refresh_token": "",
            "email": "opencode-acct",
            "type": "opencode",
            "expired": "9999-12-31T23:59:59Z",
            "account_uuid": ""
        })
        .to_string(),
    )
    .expect("write leftover opencode token");
    // Legacy flat-layout leftover, as the pre-migration layout wrote it.
    std::fs::write(
        tmp.path().join("opencode-legacy.json"),
        serde_json::json!({
            "access_token": "sk-opencode-legacy",
            "refresh_token": "",
            "email": "opencode-legacy",
            "type": "opencode",
            "expired": "9999-12-31T23:59:59Z",
            "account_uuid": ""
        })
        .to_string(),
    )
    .expect("write legacy leftover opencode token");

    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    // The leftover credential is inert: no account, no provider in admin output.
    let (status, accounts) = json_response(
        app,
        axum::http::Request::builder()
            .uri("/admin/accounts")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(accounts["providers"].get("opencode").is_none());
    // The leftovers must not be loaded into either surviving provider either.
    assert_eq!(accounts["providers"]["anthropic"]["account_count"], 0);
    assert_eq!(accounts["providers"]["codex"]["account_count"], 0);

    // ...and the files are left on disk for the operator to remove by hand.
    assert!(opencode_dir.join("opencode-acct.json").exists());
    assert!(tmp.path().join("opencode-legacy.json").exists());
}

#[tokio::test]
async fn app_enforces_configured_body_limit_and_invalid_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = config(tmp.path().to_path_buf());
    cfg.body_limit = "10b".to_string();
    let app = create_app(cfg);

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "64")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 413);
    assert_eq!(body["error"]["message"], "request body too large");

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 411);
    assert_eq!(body["error"]["message"], "missing content-length");

    let mut cfg = config(tmp.path().to_path_buf());
    cfg.body_limit = "200mb".to_string();
    let app = create_app(cfg);
    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "72")
            .body(Body::from(
                b"{\"model\":\"sonnet\",\"messages\":[{\"role\":\"user\",\"content\":\"bad\njson\"}]}".to_vec(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"]["message"], "invalid JSON body");
}

#[tokio::test]
async fn cors_allows_remote_origins() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = create_app(config(tmp.path().to_path_buf()));

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("OPTIONS")
                .uri("/v1/messages")
                .header("origin", "https://client.example.com")
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "authorization,content-type",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        axum::http::HeaderValue::from_static("*")
    );
}

#[tokio::test]
async fn v1_routes_rate_limit_by_client() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app = create_app(config(tmp.path().to_path_buf()));

    for _ in 0..60 {
        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/models")
                .header("authorization", "Bearer sk-test")
                .header("x-forwarded-for", "203.0.113.10")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
    }

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("authorization", "Bearer sk-test")
            .header("x-forwarded-for", "203.0.113.10")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, 429);
    assert_eq!(body["error"]["message"], "too many requests");
}

#[tokio::test]
async fn messages_route_forwards_anthropic_account_with_resolved_model() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["content"][0]["text"], "pong");
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].body["model"], "claude-sonnet-4-6");
    assert_eq!(calls[0].account.token.access_token, "anthropic-access");
}

#[tokio::test]
async fn messages_route_rotates_available_anthropic_accounts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (email, access_token) in [
        ("alice@example.com", "anthropic-access-alice"),
        ("bob@example.com", "anthropic-access-bob"),
    ] {
        save_token(
            tmp.path(),
            &TokenData {
                access_token: access_token.to_string(),
                refresh_token: format!("{access_token}-refresh"),
                email: email.to_string(),
                expires_at: "2030-01-01T00:00:00Z".to_string(),
                account_uuid: email.to_string(),
                provider: ProviderId::anthropic(),
                id_token: None,
                last_refresh_at: None,
                plan_type: None,
            },
        )
        .expect("save token");
    }
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    for _ in 0..2 {
        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-6",
                        "messages": [{"role": "user", "content": "reply exactly: pong"}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
    }

    let calls = upstream.calls();
    assert_eq!(calls.len(), 2);
    assert_ne!(
        calls[0].account.token.access_token,
        calls[1].account.token.access_token
    );
}

#[tokio::test]
async fn messages_route_retries_next_account_after_retryable_upstream_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (email, access_token) in [
        ("alice@example.com", "anthropic-access-alice"),
        ("bob@example.com", "anthropic-access-bob"),
    ] {
        save_token(
            tmp.path(),
            &TokenData {
                access_token: access_token.to_string(),
                refresh_token: format!("{access_token}-refresh"),
                email: email.to_string(),
                expires_at: "2030-01-01T00:00:00Z".to_string(),
                account_uuid: email.to_string(),
                provider: ProviderId::anthropic(),
                id_token: None,
                last_refresh_at: None,
                plan_type: None,
            },
        )
        .expect("save token");
    }
    let upstream = Arc::new(RetryUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["content"][0]["text"], "pong");
    let calls = upstream.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .map(|call| call.account.token.access_token.as_str())
            .collect::<Vec<_>>(),
        ["anthropic-access-alice", "anthropic-access-bob"]
    );
}

#[tokio::test]
async fn chat_completions_route_adapts_anthropic_response_to_openai() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
    assert_eq!(body["usage"]["prompt_tokens"], 1);
}

#[tokio::test]
async fn chat_completions_route_streams_anthropic_response_to_openai() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, headers, body) = raw_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "stream": true,
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert!(body.contains("\"object\":\"chat.completion.chunk\""));
    assert!(body.contains("\"content\":\"pong\""));
    assert!(body.contains("data: [DONE]"));

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/admin/accounts")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    let account = &body["providers"]["anthropic"]["accounts"][0];
    assert_eq!(account["totalSuccesses"], 1);
    assert_eq!(account["totalInputTokens"], 1);
    assert_eq!(account["totalOutputTokens"], 1);
}

#[tokio::test]
async fn responses_route_adapts_anthropic_response_to_responses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"model": "claude-sonnet-4-6", "input": "reply exactly: pong"}).to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["object"], "response");
    assert_eq!(body["output_text"], "pong");
}

#[tokio::test]
async fn responses_route_sends_web_search_and_reasoning_to_anthropic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "input": "latest docs?",
                    "tools": [{"type": "web_search"}],
                    "reasoning": {"effort": "low"}
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["object"], "response");
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body["tools"],
        json!([{"type": "web_search_20250305", "name": "web_search"}])
    );
    assert_eq!(calls[0].body["thinking"]["budget_tokens"], 4096);
}

#[tokio::test]
async fn count_tokens_route_forwards_anthropic_account_with_resolved_model() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "anthropic@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-anthropic".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages/count_tokens")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["input_tokens"], 2);
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].body["model"], "claude-sonnet-4-6");
}

#[tokio::test]
async fn messages_route_translates_anthropic_payload_for_codex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "latest docs?"}],
                    "tools": [{
                        "type": "web_search_20250305",
                        "name": "web_search",
                        "allowed_domains": ["docs.anthropic.com"]
                    }]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "ok");
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body["tools"],
        json!([{"type": "web_search", "filters": {"allowed_domains": ["docs.anthropic.com"]}}])
    );
    assert_eq!(calls[0].body["stream"], true);
    assert_eq!(calls[0].account.token.access_token, "codex-access");
}

#[tokio::test]
async fn messages_route_forwards_anthropic_tool_choice_for_codex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, _) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "weather?"}],
                    "tools": [{
                        "name": "get_weather",
                        "description": "Get weather",
                        "input_schema": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}}
                        }
                    }],
                    "tool_choice": {"type": "tool", "name": "get_weather"}
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body["tool_choice"],
        json!({"type": "function", "name": "get_weather"})
    );
    assert_eq!(calls[0].body["stream"], true);
}

#[tokio::test]
async fn chat_completions_route_adapts_codex_response_to_openai() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "reply exactly: ok"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "ok");
    assert_eq!(body["usage"]["total_tokens"], 2);
}

#[tokio::test]
async fn chat_completions_route_streams_codex_usage_to_account_stats() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, headers, body) = raw_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "stream": true,
                    "messages": [{"role": "user", "content": "reply exactly: ok"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert!(body.contains("\"content\":\"ok\""));
    assert!(body.contains("data: [DONE]"));

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/admin/accounts")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    let account = &body["providers"]["codex"]["accounts"][0];
    assert_eq!(account["totalSuccesses"], 1);
    assert_eq!(account["totalInputTokens"], 1);
    assert_eq!(account["totalOutputTokens"], 1);
}

#[tokio::test]
async fn chat_route_preserves_responses_web_search_for_codex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, _) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "latest docs?"}],
                    "responses_tools": [{"type": "web_search", "search_context_size": "low"}],
                    "responses_tool_choice": "auto"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body["tools"],
        json!([{"type": "web_search", "search_context_size": "low"}])
    );
    assert_eq!(calls[0].body["tool_choice"], "auto");
    assert_eq!(calls[0].body["stream"], true);
}

#[tokio::test]
async fn responses_route_normalizes_string_input_for_codex() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "codex-access".to_string(),
            refresh_token: "codex-refresh".to_string(),
            email: "codex@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-codex".to_string(),
            provider: ProviderId::codex(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, _) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "input": "reply exactly: ok",
                    "max_output_tokens": 32
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body["input"],
        json!([{"role": "user", "content": "reply exactly: ok"}])
    );
}

#[tokio::test]
async fn admin_accounts_lists_configured_provider_keys_loaded_at_startup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Save a static key the way `pengepul login --provider groq --key` would.
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "gsk-secret".to_string(),
            refresh_token: String::new(),
            email: "key-12345678".to_string(),
            expires_at: String::new(),
            account_uuid: "acct".to_string(),
            provider: ProviderId::generic("groq"),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save groq key");

    let mut cfg = config(tmp.path().to_path_buf());
    cfg.providers.insert(
        "groq".to_string(),
        pengepul::config::ConfiguredProvider {
            base_url: "https://api.groq.com/openai/v1".to_string(),
        },
    );
    let app = create_app(cfg);

    let (status, accounts) = json_response(
        app,
        axum::http::Request::builder()
            .uri("/admin/accounts")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(accounts["providers"]["groq"]["account_count"], 1);
    assert_eq!(
        accounts["providers"]["groq"]["accounts"][0]["email"],
        "key-12345678"
    );
    assert_eq!(
        accounts["providers"]["groq"]["accounts"][0]["available"],
        true
    );
}

#[derive(Default)]
struct GenericUpstream {
    calls: Mutex<Vec<UpstreamRequest>>,
}

impl GenericUpstream {
    fn calls(&self) -> Vec<UpstreamRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl UpstreamClient for GenericUpstream {
    fn generic_chat(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "chatcmpl_generic",
                    "object": "chat.completion",
                    "model": "llama-3.3-70b",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "pong"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                }),
            })
        })
    }

    fn generic_chat_stream(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamSseResponse {
                status: axum::http::StatusCode::OK,
                body: Box::pin(futures_util::stream::iter([
                    Ok(Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\n",
                    )),
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                ])),
            })
        })
    }

    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used in generic tests")
    }

    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used in generic tests")
    }

    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in generic tests")
    }

    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in generic tests")
    }

    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in generic tests")
    }

    fn fetch_models(
        &self,
        _kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        Box::pin(async { Ok(FetchedModels::new(Vec::new())) })
    }
}

fn groq_key_token() -> TokenData {
    TokenData {
        access_token: "gsk-secret".to_string(),
        refresh_token: String::new(),
        email: "key-12345678".to_string(),
        expires_at: String::new(),
        account_uuid: "acct-groq".to_string(),
        provider: ProviderId::generic("groq"),
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
    }
}

fn config_with_groq(auth_dir: PathBuf) -> Config {
    let mut cfg = config(auth_dir);
    cfg.providers.insert(
        "groq".to_string(),
        pengepul::config::ConfiguredProvider {
            base_url: "https://api.groq.com/openai/v1".to_string(),
        },
    );
    cfg
}

#[tokio::test]
async fn chat_completions_with_a_configured_model_reaches_the_endpoint_with_a_bare_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1024")
            .body(Body::from(
                json!({
                    "model": "groq/llama-3.3-70b",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
    let calls = upstream.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].body["model"], "llama-3.3-70b");
    assert_eq!(calls[0].account.token.access_token, "gsk-secret");
    assert_eq!(calls[0].account.provider.id.as_ref(), "groq");
}

#[tokio::test]
async fn a_model_prefix_no_configured_provider_claims_is_unknown() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1024")
            .body(Body::from(
                json!({
                    "model": "mistral/mistral-large",
                    "messages": [{"role": "user", "content": "hi"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 400);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown model")
    );
    assert!(upstream.calls().is_empty());
}

#[tokio::test]
async fn generic_models_answer_501_on_messages_responses_and_count_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    for (uri, body) in [
        (
            "/v1/messages",
            json!({
                "model": "groq/llama-3.3-70b",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}]
            }),
        ),
        (
            "/v1/responses",
            json!({"model": "groq/llama-3.3-70b", "input": "hi"}),
        ),
        (
            "/v1/messages/count_tokens",
            json!({
                "model": "groq/llama-3.3-70b",
                "messages": [{"role": "user", "content": "hi"}]
            }),
        ),
    ] {
        let (status, body) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1024")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;

        assert_eq!(status, 501, "{uri} answered");
        assert_eq!(body["error"]["type"], "unsupported_endpoint_for_provider");
    }
    assert!(upstream.calls().is_empty());
}

struct FirstKeyFailsUpstream {
    calls: Mutex<Vec<UpstreamRequest>>,
}

impl UpstreamClient for FirstKeyFailsUpstream {
    fn generic_chat(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let fail = request.account.token.access_token == "gsk-secret";
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async move {
            Ok(UpstreamJsonResponse {
                status: if fail {
                    axum::http::StatusCode::TOO_MANY_REQUESTS
                } else {
                    axum::http::StatusCode::OK
                },
                body: if fail {
                    json!({"error": {"message": "rate limited"}})
                } else {
                    json!({
                        "id": "chatcmpl_ok",
                        "object": "chat.completion",
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
                    })
                },
            })
        })
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("stream not used in generic failover test")
    }

    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used")
    }

    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used")
    }

    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used")
    }

    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used")
    }

    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used")
    }

    fn fetch_models(
        &self,
        _kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        Box::pin(async { Ok(FetchedModels::new(Vec::new())) })
    }
}

#[tokio::test]
async fn generic_failover_moves_between_keys_of_the_same_endpoint_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save first key");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "gsk-second".to_string(),
            refresh_token: String::new(),
            email: "key-87654321".to_string(),
            expires_at: String::new(),
            account_uuid: "acct-groq-2".to_string(),
            provider: ProviderId::generic("groq"),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save second key");

    let upstream = Arc::new(FirstKeyFailsUpstream {
        calls: Mutex::new(Vec::new()),
    });
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1024")
            .body(Body::from(
                json!({
                    "model": "groq/llama-3.3-70b",
                    "messages": [{"role": "user", "content": "hi"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "ok");
    let calls = upstream.calls.lock().expect("calls lock");
    let keys: Vec<_> = calls
        .iter()
        .map(|c| c.account.token.access_token.clone())
        .collect();
    assert_eq!(keys, ["gsk-secret", "gsk-second"]);
    assert!(
        calls
            .iter()
            .all(|c| c.account.provider.id.as_ref() == "groq")
    );
}

#[derive(Default)]
struct ModelsUpstream {
    calls: Mutex<Vec<ProviderKind>>,
}

impl UpstreamClient for ModelsUpstream {
    fn fetch_models(
        &self,
        kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        self.calls.lock().expect("calls lock").push(kind);
        let ids = if kind == ProviderKind::Generic {
            vec!["llama-3.3-70b-versatile".to_string()]
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(FetchedModels::new(ids)) })
    }
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("chat not used in models test")
    }
    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("stream not used in models test")
    }
    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used in models test")
    }
    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used in models test")
    }
    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in models test")
    }
    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in models test")
    }
    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in models test")
    }
}

#[tokio::test]
async fn v1_models_advertises_fetched_configured_models_with_their_prefix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(ModelsUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    // The catalog refresh loop runs on a spawned task right after app creation;
    // poll the route until the fetch lands (bounded, then fail).
    let mut ids = Vec::new();
    for _ in 0..50 {
        let (status, body) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .uri("/v1/models")
                .header("authorization", "Bearer sk-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        ids = body["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(ToOwned::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if ids.iter().any(|id| id == "groq/llama-3.3-70b-versatile") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        ids.iter().any(|id| id == "groq/llama-3.3-70b-versatile"),
        "advertised ids: {ids:?}"
    );
}
