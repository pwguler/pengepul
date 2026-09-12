use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::{Body, Bytes};
use http_body_util::BodyExt;
use pengepul::app::{
    CliVersionsFuture, ModelsFuture, UpstreamClient, UpstreamFuture, UpstreamJsonResponse,
    UpstreamRequest, UpstreamSseFuture, UpstreamSseResponse, create_app, create_app_with_upstream,
};
use pengepul::cloaking_versions::CliVersions;
use pengepul::config::{
    BodyLimit, CloakingConfig, Config, DEFAULT_BODY_LIMIT_BYTES, DebugMode, TimeoutConfig,
};
use pengepul::models::{FetchedModels, ModelMetadata, ModelPricing};
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
        body_limit: BodyLimit::Limited(DEFAULT_BODY_LIMIT_BYTES),
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
    cfg.body_limit = BodyLimit::Limited(10);
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
    cfg.body_limit = BodyLimit::Limited(DEFAULT_BODY_LIMIT_BYTES);
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

/// A JSON body of exactly `size` bytes, valid, with `messages` empty so the relay's own
/// validation answers if the bytes are ever read. `messages: []` is what keeps these probes
/// off the upstream.
fn padded_body(size: usize) -> Vec<u8> {
    let mut body = String::from(r#"{"model":"m","messages":[]}"#);
    assert!(size >= body.len(), "{size} is too small to be this body");
    body.push_str(&" ".repeat(size - body.len()));
    body.into_bytes()
}

/// POST one of those bodies to `uri`, declaring its true length, and return the raw text.
/// Raw rather than JSON because the extractor's rejection is plain text, and asking for JSON
/// there panics before any assertion runs — which is how the first version of this test
/// failed for the wrong reason.
async fn post_padded(app: axum::Router, uri: &str, size: usize) -> (u16, String) {
    let body = padded_body(size);
    let (status, _, text) = raw_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", body.len().to_string())
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    (status, text)
}

/// The regression this pins: `body-limit` never reached axum's body buffering, so the
/// extractor applied its own 2 MiB default to a body the operator had explicitly allowed,
/// before any handler ran.
#[tokio::test]
async fn a_body_above_the_extractor_default_still_reaches_the_handler() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = config(tmp.path().to_path_buf());
    cfg.body_limit = BodyLimit::Limited(DEFAULT_BODY_LIMIT_BYTES);
    let app = create_app(cfg);

    // Past axum's 2 MiB default. The relay's own validation answering is the proof the bytes
    // were read: a limit inside the extractor would have produced its own 413 first.
    let (status, body) = post_padded(app, "/v1/chat/completions", 3 * 1024 * 1024).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("messages is required"),
        "the extractor answered instead of the handler: {body}"
    );
}

/// The half the test above cannot see, and the more important one.
///
/// A bare `413` says nothing about *which* limit produced it, so a fix that took
/// `max(configured, 2 MiB)` — or one that limited only the first `/v1` nest — would leave
/// the reported bug alive and every test above still green. This pins the boundary itself,
/// on a limit nowhere near any default, and on both mounts a client can arrive through.
/// `README.md` documents `http://host:port/v1` as the base URL, which is how an Anthropic
/// SDK reaches `/v1/v1/messages`: the second nest is not a curiosity.
#[tokio::test]
async fn the_extractor_enforces_the_configured_limit_on_every_mount() {
    const LIMIT: usize = 1024;
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = config(tmp.path().to_path_buf());
    cfg.body_limit = BodyLimit::Limited(LIMIT as u64);
    let app = create_app(cfg);

    // Every nested POST route, on both mounts. Four of the eight was enough to catch the
    // mutations above, but not enough to pin what AC-3 claims: splitting the router so that
    // `/responses` and `/messages/count_tokens` lose the layer left all 470 tests green.
    for uri in [
        "/v1/chat/completions",
        "/v1/messages",
        "/v1/responses",
        "/v1/messages/count_tokens",
        "/v1/v1/chat/completions",
        "/v1/v1/messages",
        "/v1/v1/responses",
        "/v1/v1/messages/count_tokens",
    ] {
        // At the limit the bytes are read and the relay's own validation answers, which is a
        // 400 for this body. Pinned exactly rather than as "not 413": a 401 or a 404 would
        // satisfy the weaker form while saying nothing about the limit.
        let (at, body) = post_padded(app.clone(), uri, LIMIT).await;
        assert_eq!(
            at, 400,
            "{uri} did not read a body of exactly the configured limit: {body}"
        );

        // One byte more is turned away by the extractor, so the handler's message must not
        // appear: if it does, the configured number was not what bound the read.
        let (over, body) = post_padded(app.clone(), uri, LIMIT + 1).await;
        assert_eq!(over, 413, "{uri} at limit + 1: {body}");
        assert!(
            !body.contains("request body too large"),
            "{uri} was bound by the handler, not by the configured limit: {body}"
        );
    }
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

    // Two conversations, so two different cacheable prefixes. Identical
    // requests deliberately do not rotate any more: they share a prefix,
    // and moving them between accounts throws away the cache each just
    // paid to write (ADR-0017).
    for system in ["you are a poet", "you are a lawyer"] {
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
                        "system": system,
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
async fn one_conversation_stays_on_one_account() {
    // Each upstream account holds its own prompt cache, so alternating
    // accounts mid-conversation throws the prefix away on every turn. The
    // session the client already sends is what pins it.
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

    for _ in 0..4 {
        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
                .header("x-claude-code-session-id", "conversation-a")
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
    assert_eq!(calls.len(), 4);
    let served: std::collections::BTreeSet<&str> = calls
        .iter()
        .map(|call| call.account.token.access_token.as_str())
        .collect();
    assert_eq!(
        served.len(),
        1,
        "one conversation was split across {} accounts: {served:?}",
        served.len()
    );
}

#[tokio::test]
async fn a_different_conversation_still_takes_the_next_account() {
    // Affinity is a preference, not a pin on the pool: two conversations
    // still spread across the accounts.
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

    for session in ["conversation-a", "conversation-b"] {
        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
                .header("x-claude-code-session-id", session)
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
    assert_ne!(
        calls[0].account.token.access_token, calls[1].account.token.access_token,
        "two conversations landed on the same account"
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
async fn chat_completions_route_records_generic_stream_usage() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    let (status, _headers, body) = raw_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "groq/llama-3.3-70b",
                    "stream": true,
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
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
    let account = &body["providers"]["groq"]["accounts"][0];
    assert_eq!(account["totalSuccesses"], 1);
    // The fake upstream's final chunk carries usage before [DONE]; the
    // relay must read it the way it reads the non-streamed body.
    //
    // 17, not 21: an OpenAI-dialect `prompt_tokens` is the whole prompt and
    // `cached_tokens` sits inside it, so a cache read is not input (ADR-0023,
    // docs/specs/cache-counter-semantics.md AC-2).
    assert_eq!(account["totalInputTokens"], 17);
    assert_eq!(account["totalOutputTokens"], 22);
    assert_eq!(account["totalCacheReadInputTokens"], 4);
}

/// An upstream whose `usage` is whatever the test chose, served on both JSON routes this
/// relay records from. It exists so one set of numbers can be sent through two Providers
/// and the recorded counters compared: the cache counts sit *beside* `input_tokens` on
/// the Anthropic wire and *inside* the prompt count on the `OpenAI` one, and that placement
/// is the only thing under test.
struct ChosenUsageUpstream {
    usage: Value,
}

impl UpstreamClient for ChosenUsageUpstream {
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "chatcmpl_usage",
                    "object": "chat.completion",
                    "model": "llama-3.3-70b",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "pong"},
                        "finish_reason": "stop"
                    }],
                    "usage": usage
                }),
            })
        })
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("the usage tests do not stream")
    }

    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-6",
                    "content": [{"type": "text", "text": "pong"}],
                    "stop_reason": "end_turn",
                    "usage": usage
                }),
            })
        })
    }

    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("the usage tests do not stream")
    }

    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("the usage tests do not count tokens")
    }

    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let usage = self.usage.clone();
        Box::pin(async move {
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
                        "content": [{"type": "output_text", "text": "pong", "annotations": []}]
                    }],
                    "usage": usage
                }),
            })
        })
    }

    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("the usage tests do not stream")
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

/// One account, saved where the relay loads its pool from.
fn save_provider_token(dir: &std::path::Path, provider: ProviderId, email: &str) {
    save_token(
        dir,
        &TokenData {
            access_token: format!("access-{email}"),
            refresh_token: format!("refresh-{email}"),
            email: email.to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: format!("acct-{email}"),
            provider,
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
}

/// What `status` and `usage` call carried load: every token the upstream processed,
/// counted once. It is the number this whole change exists to make dialect-independent,
/// so it is asserted from the counters rather than recomputed in the view.
fn carried_load(account: &Value) -> i64 {
    [
        "totalInputTokens",
        "totalOutputTokens",
        "totalCacheReadInputTokens",
        "totalCacheCreationInputTokens",
    ]
    .iter()
    .map(|field| account[*field].as_i64().unwrap_or(0))
    .sum()
}

/// The counters of the pool's one account, as `/admin/accounts` reports them.
async fn recorded_usage(app: axum::Router, pool: &str) -> Value {
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
    body["providers"][pool]["accounts"][0].clone()
}

#[tokio::test]
async fn an_anthropic_usage_keeps_its_input_beside_its_cache_counts() {
    // Anthropic's `input_tokens` is already the uncached tail — the tokens after the last
    // cache breakpoint — so the two cache counters beside it must not be subtracted a
    // second time. The numbers are the same shape as the Codex test below and the answer
    // is the opposite one, because the dialect decides and not the field name (AC-3).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_provider_token(tmp.path(), ProviderId::anthropic(), "anthropic@example.com");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "input_tokens": 100,
            "output_tokens": 5,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1
        }),
    });
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

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

    let account = recorded_usage(app, "anthropic").await;
    assert_eq!(
        account["totalInputTokens"], 100,
        "an Anthropic input count was treated as though it held the cache"
    );
    assert_eq!(account["totalCacheReadInputTokens"], 2);
    assert_eq!(account["totalCacheCreationInputTokens"], 1);
    // 100 + 5 + 2 + 1: this 103-token prompt is counted once. The OpenAI-dialect
    // sibling describes the same prompt as `input_tokens: 103` with the cache inside
    // it and must land on the same total (AC-8).
    assert_eq!(carried_load(&account), 108);
}

#[tokio::test]
async fn a_codex_usage_records_only_the_uncached_input() {
    // The Responses wire reports `input_tokens` as the whole input, with
    // `input_tokens_details.cached_tokens` a slice of it: 40 of these 100 tokens were
    // served from cache and 60 were not (AC-1, and AC-2 on the Responses shape).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_provider_token(tmp.path(), ProviderId::codex(), "codex@example.com");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "input_tokens": 100,
            "output_tokens": 5,
            "total_tokens": 105,
            "input_tokens_details": {"cached_tokens": 40}
        }),
    });
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "input": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let account = recorded_usage(app, "codex").await;
    assert_eq!(account["totalInputTokens"], 60);
    assert_eq!(account["totalCacheReadInputTokens"], 40);
}

#[tokio::test]
async fn more_cached_tokens_than_input_records_no_negative_input() {
    // An upstream that reports more cached tokens than prompt tokens is broken, and a
    // token counter is the wrong place to notice. What it must not do is record a
    // negative input for every later total to inherit (AC-7).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_provider_token(tmp.path(), ProviderId::codex(), "codex@example.com");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "input_tokens": 10,
            "output_tokens": 1,
            "total_tokens": 11,
            "input_tokens_details": {"cached_tokens": 5000}
        }),
    });
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "input": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let account = recorded_usage(app, "codex").await;
    assert_eq!(account["totalInputTokens"], 0);
    assert_eq!(account["totalCacheReadInputTokens"], 5000);
}

#[tokio::test]
async fn a_codex_cache_write_is_recorded_as_a_cache_write() {
    // GPT-5.6 writes cache and reports it *inside* the input count, at
    // `input_tokens_details.cache_write_tokens`. Recording it is what makes the two input
    // categories the vendor bills separately — cached and written — visible at all, and
    // subtracting it beside the cache read is what keeps these 103 prompt tokens counted
    // once (AC-6, AC-8). Its sibling on the other wire is
    // `an_anthropic_usage_keeps_its_input_beside_its_cache_counts`, which describes the
    // same 103-token prompt and must reach the same 108.
    let tmp = tempfile::tempdir().expect("tempdir");
    save_provider_token(tmp.path(), ProviderId::codex(), "codex@example.com");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "input_tokens": 103,
            "output_tokens": 5,
            "total_tokens": 108,
            "input_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 1}
        }),
    });
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "gpt-5.4",
                    "input": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);

    let account = recorded_usage(app, "codex").await;
    assert_eq!(
        account["totalInputTokens"], 100,
        "the cache write was left inside the input count"
    );
    assert_eq!(account["totalCacheReadInputTokens"], 2);
    assert_eq!(
        account["totalCacheCreationInputTokens"], 1,
        "a Responses cache write was not recorded at all"
    );
    assert_eq!(carried_load(&account), 108);
}

#[tokio::test]
async fn a_chat_cache_write_is_recorded_as_a_cache_write() {
    // OpenRouter publishes `cache_write_tokens` in the *chat* shape, beside `cached_tokens`
    // in the same `prompt_tokens_details` object, so a chat-only reader loses it.
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "prompt_tokens": 103,
            "completion_tokens": 5,
            "total_tokens": 108,
            "prompt_tokens_details": {"cached_tokens": 2, "cache_write_tokens": 1}
        }),
    });
    let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
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

    let account = recorded_usage(app, "groq").await;
    assert_eq!(account["totalInputTokens"], 100);
    assert_eq!(account["totalCacheReadInputTokens"], 2);
    assert_eq!(
        account["totalCacheCreationInputTokens"], 1,
        "a chat cache write was not recorded at all"
    );
    assert_eq!(carried_load(&account), 108);
}

#[tokio::test]
async fn a_flat_cached_tokens_is_read_beside_the_nested_one() {
    // Moonshot/Kimi define `cached_tokens` as a sibling of `prompt_tokens` — there is no
    // `prompt_tokens_details` in that schema at all — so a nested-only reader records 0
    // cache reads for every Kimi request (AC-4).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "prompt_tokens": 21,
            "completion_tokens": 5,
            "total_tokens": 26,
            "cached_tokens": 4
        }),
    });
    let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
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

    let account = recorded_usage(app, "groq").await;
    assert_eq!(
        account["totalCacheReadInputTokens"], 4,
        "a flat cached_tokens was not read"
    );
    assert_eq!(account["totalInputTokens"], 17);
}

#[tokio::test]
async fn a_deepseek_cache_hit_becomes_the_cache_read_and_the_miss_the_input() {
    // DeepSeek publishes no `cached_tokens` at all: it splits the input into
    // `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`. Reading the hit as a cache
    // read leaves the miss as the input, which is the same number DeepSeek reports for it
    // — the split is complementary, so `prompt_tokens - hit = miss` (AC-5).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "prompt_tokens": 100,
            "completion_tokens": 5,
            "total_tokens": 105,
            "prompt_cache_hit_tokens": 90,
            "prompt_cache_miss_tokens": 10
        }),
    });
    let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
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

    let account = recorded_usage(app, "groq").await;
    assert_eq!(
        account["totalCacheReadInputTokens"], 90,
        "a DeepSeek cache hit was recorded as 0"
    );
    assert_eq!(
        account["totalInputTokens"], 10,
        "the uncached input is the miss count, and 10 is what DeepSeek calls it"
    );
}

#[tokio::test]
async fn output_tokens_include_reasoning_where_the_vendor_counts_it_outside() {
    // `output_tokens` means every token the model generated, so `carried load` is the
    // `total_tokens` the vendor billed. xAI's chat dialect counts reasoning *outside*
    // `completion_tokens` and inside `total_tokens` (its own example: 32 + 9 + 110 = 151),
    // so 9 must become 119 or carried load reads 41 against a billed 151. OpenAI counts
    // reasoning *inside* `completion_tokens`, so folding there would make carried 135
    // against a billed 120 (AC-10).
    let cases = [
        (
            "xai chat",
            json!({
                "prompt_tokens": 32,
                "completion_tokens": 9,
                "total_tokens": 151,
                "prompt_tokens_details": {"cached_tokens": 8},
                "completion_tokens_details": {"reasoning_tokens": 110}
            }),
            119,
            110,
            151,
        ),
        (
            "openai",
            json!({
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "completion_tokens_details": {"reasoning_tokens": 15}
            }),
            20,
            15,
            120,
        ),
    ];
    for (label, usage, output, reasoning, billed) in cases {
        let tmp = tempfile::tempdir().expect("tempdir");
        save_token(tmp.path(), &groq_key_token()).expect("save groq key");
        let upstream = Arc::new(ChosenUsageUpstream { usage });
        let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
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
        assert_eq!(status, 200, "[{label}]");

        let account = recorded_usage(app, "groq").await;
        assert_eq!(account["totalOutputTokens"], output, "[{label}] out");
        assert_eq!(
            account["totalReasoningOutputTokens"], reasoning,
            "[{label}] reasoning"
        );
        assert_eq!(
            carried_load(&account),
            billed,
            "[{label}] carried load is not the total the vendor billed"
        );
    }
}

#[tokio::test]
async fn an_anthropic_iterations_array_is_not_summed_into_the_counters() {
    // Anthropic's usage also carries `iterations[]`, each entry repeating the whole set of
    // counters for one internal pass. Nothing reads it, and that is the correct behaviour:
    // summing it would multiply every count by the number of passes. The numbers here are
    // the ones `an_anthropic_usage_keeps_its_input_beside_its_cache_counts` uses, so the
    // two tests disagree the moment a reader starts walking the array (AC-9).
    let tmp = tempfile::tempdir().expect("tempdir");
    save_provider_token(tmp.path(), ProviderId::anthropic(), "anthropic@example.com");
    let pass = |kind: &str| {
        json!({
            "input_tokens": 100,
            "output_tokens": 5,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1,
            "type": kind
        })
    };
    let upstream = Arc::new(ChosenUsageUpstream {
        usage: json!({
            "input_tokens": 100,
            "output_tokens": 5,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1,
            "iterations": [pass("message"), pass("compaction")]
        }),
    });
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

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

    let account = recorded_usage(app, "anthropic").await;
    assert_eq!(
        account["totalInputTokens"], 100,
        "the iterations array was summed into the input count"
    );
    assert_eq!(account["totalOutputTokens"], 5);
    assert_eq!(account["totalCacheReadInputTokens"], 2);
    assert_eq!(account["totalCacheCreationInputTokens"], 1);
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
                    Ok(Bytes::from_static(
                        b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":22,\"prompt_tokens_details\":{\"cached_tokens\":4},\"completion_tokens_details\":{\"reasoning_tokens\":7}}}\n\n",
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

fn static_key_token(provider: &str, label: &str) -> TokenData {
    TokenData {
        access_token: format!("secret-{provider}-{label}"),
        refresh_token: String::new(),
        email: label.to_string(),
        expires_at: String::new(),
        account_uuid: format!("acct-{label}"),
        provider: ProviderId::generic(provider),
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
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

fn config_with_static_provider(name: &str, auth_dir: PathBuf) -> Config {
    let mut cfg = config(auth_dir);
    cfg.providers.insert(
        name.to_string(),
        pengepul::config::ConfiguredProvider {
            base_url: format!("https://{name}.example/v1"),
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
async fn a_messages_request_reaches_a_generic_endpoint_as_chat_completions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    let (status, body) = json_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/messages")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1024")
            .body(Body::from(
                json!({
                    "model": "groq/llama-3.3-70b",
                    "max_tokens": 16,
                    "system": "be brief",
                    "messages": [{"role": "user", "content": "hi"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    // What went up is the endpoint's own dialect: a system message rather
    // than a `system` field, and the bare model id.
    let sent = upstream.calls().first().cloned().expect("one call");
    assert_eq!(sent.body["model"], "llama-3.3-70b");
    assert!(sent.body.get("system").is_none(), "{}", sent.body);
    assert_eq!(sent.body["messages"][0]["role"], "system");
    assert_eq!(sent.body["messages"][0]["content"], "be brief");
    assert_eq!(sent.body["messages"][1]["role"], "user");
    assert_eq!(sent.body["max_tokens"], 16);
    // And what came back is Messages again, which is what the client reads.
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["stop_reason"], "end_turn");
}

#[tokio::test]
async fn generic_models_answer_501_on_responses_and_count_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app =
        create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream.clone());

    // Messages is served now, by translation onto Chat Completions. These
    // two are not: Responses has no client asking for it, and count_tokens
    // is anthropic's own endpoint.
    for (uri, body) in [
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
        let mut metadata = BTreeMap::new();
        if kind == ProviderKind::Generic {
            metadata.insert(
                "llama-3.3-70b-versatile".to_string(),
                ModelMetadata {
                    context_window: Some(131_072),
                    max_output_tokens: Some(32_768),
                    input_modalities: Some(vec!["text".to_string()]),
                    reasoning: None,
                    pricing: Some(ModelPricing {
                        input_per_million: Some(0.59),
                        output_per_million: Some(0.79),
                        cache_read_per_million: None,
                        cache_write_per_million: None,
                    }),
                },
            );
        }
        Box::pin(async move { Ok(FetchedModels::with_metadata(ids, metadata)) })
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

#[derive(Default)]
struct BillingFailoverUpstream {
    /// account labels that have been called, in order
    calls: Mutex<Vec<String>>,
}

impl UpstreamClient for BillingFailoverUpstream {
    fn generic_chat(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let email = request.account.token.email.clone();
        let mut calls = self.calls.lock().expect("calls lock");
        let call_index = calls.len() + 1;
        calls.push(email);
        drop(calls);
        Box::pin(async move {
            if call_index == 1 {
                Ok(UpstreamJsonResponse {
                    status: axum::http::StatusCode::BAD_REQUEST,
                    body: json!({
                        "error": {
                            "message": "You have insufficient credits to make this request. Please purchase more credits to continue using the service.",
                            "type": "invalid_request_error",
                            "code": "BAD_REQUEST"
                        }
                    }),
                })
            } else {
                Ok(UpstreamJsonResponse {
                    status: axum::http::StatusCode::OK,
                    body: json!({
                        "id": "chatcmpl_billfailover",
                        "object": "chat.completion",
                        "model": "gpt-5.6-sol",
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "pong"}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    }),
                })
            }
        })
    }
    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("stream not used in billing failover test")
    }
    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used in billing failover test")
    }
    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used in billing failover test")
    }
    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in billing failover test")
    }
    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in billing failover test")
    }
    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in billing failover test")
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
async fn billing_scoped_upstream_rejection_fails_over_to_the_next_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &static_key_token("commandcode", "key-90445c90"))
        .expect("save first key");
    save_token(tmp.path(), &static_key_token("commandcode", "key-d792179a"))
        .expect("save second key");
    let upstream = Arc::new(BillingFailoverUpstream::default());
    let app = create_app_with_upstream(
        config_with_static_provider("commandcode", tmp.path().to_path_buf()),
        upstream.clone(),
    );

    let (status, body) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1024")
            .body(Body::from(
                json!({
                    "model": "commandcode/gpt-5.6-sol",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    // The first account is out of credits; the request still succeeds on the sibling.
    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
    let calls = upstream.calls.lock().expect("calls lock").clone();
    assert_eq!(
        calls.len(),
        2,
        "failover should have reached the second account"
    );
    assert_ne!(
        calls[0], calls[1],
        "failover must move to a different account"
    );
    // the drained account is marked billing, so rotation skips it for a long while
    let (admin_status, admin) = json_response(
        app,
        axum::http::Request::builder()
            .uri("/admin/accounts")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(admin_status, 200);
    let drained = admin["providers"]["commandcode"]["accounts"]
        .as_array()
        .and_then(|accounts| accounts.iter().find(|a| a["email"] == calls[0]))
        .expect("drained account in admin output");
    assert_eq!(drained["failureCount"], 1);
    assert!(
        drained["lastError"]
            .as_str()
            .is_some_and(|e| e.starts_with("billing:")),
        "failure must be classified billing, got {:?}",
        drained["lastError"]
    );
    // the sibling that served the request shows no failure
    let served = admin["providers"]["commandcode"]["accounts"]
        .as_array()
        .and_then(|accounts| accounts.iter().find(|a| a["email"] == calls[1]))
        .expect("serving account in admin output");
    assert_eq!(served["failureCount"], 0);
    // One request, one outcome: the billing marker adds the cooldown, not
    // a second failure (ARCHITECTURE, "A counter counts outcomes, never
    // attempts"). Exact counts, not balance: a double-count now adds a
    // request and a failure together, so it balances and only the tuple
    // catches it.
    let requests = drained["totalRequests"].as_i64().expect("requests");
    let ok = drained["totalSuccesses"].as_i64().expect("ok");
    let failed = drained["totalFailures"].as_i64().expect("failed");
    assert_eq!(
        (requests, ok, failed),
        (1, 0, 1),
        "a billing rejection counted twice: {requests} requests, {ok} ok, {failed} failed"
    );
    // The cooldown it justifies is still applied.
    assert_eq!(
        drained["available"], false,
        "the drained account kept serving"
    );
}

#[tokio::test]
async fn v1_models_carries_per_model_metadata_additively() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let app = create_app_with_upstream(
        config_with_groq(tmp.path().to_path_buf()),
        Arc::new(ModelsUpstream::default()),
    );

    let request = |_app: &axum::Router| {
        axum::http::Request::builder()
            .uri("/v1/models")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap()
    };
    let mut entry = Value::Null;
    for _ in 0..50 {
        let (status, body) = json_response(app.clone(), request(&app)).await;
        assert_eq!(status, 200);
        entry = body["data"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["id"] == "groq/llama-3.3-70b-versatile")
            })
            .cloned()
            .unwrap_or(Value::Null);
        if !entry.is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // The base OpenAI fields are untouched...
    assert_eq!(entry["id"], "groq/llama-3.3-70b-versatile");
    assert_eq!(entry["object"], "model");
    assert_eq!(entry["owned_by"], "groq");
    // ...the metadata the upstream published is present...
    assert_eq!(entry["context_window"], 131_072);
    assert_eq!(entry["max_output_tokens"], 32_768);
    assert_eq!(entry["input_modalities"], json!(["text"]));
    assert_eq!(entry["pricing"]["input_per_million"], 0.59);
    assert_eq!(entry["pricing"]["output_per_million"], 0.79);
    // ...and rates the upstream did not publish are omitted, not zeroed.
    assert!(entry["pricing"].get("cache_read_per_million").is_none());
    assert!(entry["pricing"].get("cache_write_per_million").is_none());
}

#[tokio::test]
async fn admin_reload_picks_up_a_newly_saved_key_for_a_configured_provider() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save first key");
    let upstream = Arc::new(GenericUpstream::default());
    let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

    // A second key lands on disk while the relay is running.
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

    let (status, reloaded) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/admin/reload")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        reloaded["reloaded"]["groq"]["added"],
        json!(["key-87654321"])
    );

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
    assert_eq!(accounts["providers"]["groq"]["account_count"], 2);
}

#[tokio::test]
async fn the_api_is_served_at_v1_and_at_a_doubled_v1() {
    // An Anthropic SDK pointed at a `.../v1` base URL asks for `/v1/v1/messages`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);
    let get = |uri: &str| {
        axum::http::Request::builder()
            .uri(uri)
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap()
    };
    let post = |uri: &str| {
        let body =
            json!({"model": "claude-opus-5", "max_tokens": 5, "messages": [{"role": "user", "content": "hi"}]})
                .to_string();
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", body.len().to_string())
            .body(Body::from(body))
            .unwrap()
    };

    let (single, _) = json_response(app.clone(), get("/v1/models")).await;
    let (doubled, _) = json_response(app.clone(), get("/v1/v1/models")).await;
    assert_eq!(single, 200);
    assert_eq!(doubled, 200);

    let (single, _) = json_response(app.clone(), post("/v1/messages")).await;
    let (doubled, _) = json_response(app.clone(), post("/v1/v1/messages")).await;
    assert_eq!(single, doubled, "both prefixes reach the same handler");

    let response = app
        .oneshot(get("/v1/v1/v1/models"))
        .await
        .expect("response");
    assert_eq!(
        response.status().as_u16(),
        404,
        "only one duplicate is tolerated"
    );
}

struct VersionedUpstream {
    inner: FakeUpstream,
    fetches: Mutex<u32>,
    result: CliVersions,
}

impl VersionedUpstream {
    fn shipping(claude: &str, codex: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FakeUpstream::default(),
            fetches: Mutex::new(0),
            result: CliVersions {
                claude: claude.parse().ok(),
                codex: codex.parse().ok(),
            },
        })
    }

    async fn fetched(&self) {
        for _ in 0..100 {
            if *self.fetches.lock().expect("fetches lock") > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the first fetch runs at startup");
    }
}

impl UpstreamClient for VersionedUpstream {
    fn generic_chat(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.inner.generic_chat(request)
    }
    fn generic_chat_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        self.inner.generic_chat_stream(request)
    }
    fn anthropic_messages(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.inner.anthropic_messages(request)
    }
    fn anthropic_messages_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        self.inner.anthropic_messages_stream(request)
    }
    fn anthropic_count_tokens(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.inner.anthropic_count_tokens(request)
    }
    fn codex_responses(&self, request: UpstreamRequest) -> UpstreamFuture {
        self.inner.codex_responses(request)
    }
    fn codex_responses_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        self.inner.codex_responses_stream(request)
    }
    fn fetch_models(
        &self,
        kind: ProviderKind,
        account: AvailableAccount,
        config: Arc<Config>,
    ) -> ModelsFuture {
        self.inner.fetch_models(kind, account, config)
    }
    fn fetch_cli_versions(&self) -> CliVersionsFuture {
        *self.fetches.lock().expect("fetches lock") += 1;
        let result = self.result.clone();
        Box::pin(async { Ok(result) })
    }
}

fn anthropic_account(dir: &std::path::Path) {
    save_token(
        dir,
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
}

fn messages_request() -> axum::http::Request<Body> {
    let body =
        json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "hi"}]})
            .to_string();
    axum::http::Request::builder()
        .method("POST")
        .uri("/v1/messages")
        .header("authorization", "Bearer sk-test")
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn fetched_cli_versions_reach_the_upstream_request_and_the_cache() {
    // AC-1, AC-2 (fetched above the configured floor), AC-5 (write side)
    let tmp = tempfile::tempdir().expect("tempdir");
    anthropic_account(tmp.path());
    let upstream = VersionedUpstream::shipping("2.1.251", "0.151.0");
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());
    upstream.fetched().await;

    let (status, _) = json_response(app, messages_request()).await;
    assert_eq!(status, 200);
    let call = &upstream.inner.calls()[0];
    assert_eq!(call.config.cloaking.cli_version, "2.1.251");
    assert_eq!(
        call.config
            .cloaking
            .codex
            .get("cli-version")
            .map(String::as_str),
        Some("0.151.0")
    );

    let cached = CliVersions::load(&tmp.path().join("cloaking-versions.json"));
    assert_eq!(cached.claude, "2.1.251".parse().ok());
    assert_eq!(cached.codex, "0.151.0".parse().ok());
}

#[tokio::test]
async fn a_configured_version_above_the_fetched_one_is_kept() {
    // AC-2 (configured above fetched)
    let tmp = tempfile::tempdir().expect("tempdir");
    anthropic_account(tmp.path());
    let upstream = VersionedUpstream::shipping("2.1.251", "0.151.0");
    let mut pinned = config(tmp.path().to_path_buf());
    pinned.cloaking.cli_version = "9.0.0".to_string();
    let app = create_app_with_upstream(pinned, upstream.clone());
    upstream.fetched().await;

    let (status, _) = json_response(app, messages_request()).await;
    assert_eq!(status, 200);
    assert_eq!(
        upstream.inner.calls()[0].config.cloaking.cli_version,
        "9.0.0"
    );
}

#[tokio::test]
async fn a_failed_fetch_keeps_the_cached_or_baked_versions() {
    // AC-4, AC-5 (read side): FakeUpstream has no version source, so every fetch fails.
    let tmp = tempfile::tempdir().expect("tempdir");
    anthropic_account(tmp.path());
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let (status, _) = json_response(app, messages_request()).await;
    assert_eq!(status, 200, "a failed fetch does not stop the relay");
    assert_eq!(upstream.calls()[0].config.cloaking.cli_version, "2.1.88");

    CliVersions {
        claude: "2.1.200".parse().ok(),
        codex: None,
    }
    .save(&tmp.path().join("cloaking-versions.json"))
    .expect("seed cache");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let (status, _) = json_response(app, messages_request()).await;
    assert_eq!(status, 200);
    assert_eq!(
        upstream.calls()[0].config.cloaking.cli_version,
        "2.1.200",
        "the cache is effective before any fetch"
    );
}

#[tokio::test]
async fn a_fetch_that_parses_to_nothing_keeps_the_known_versions() {
    // AC-4: a registry body that yields no version must not move what is known,
    // in memory or on disk.
    let tmp = tempfile::tempdir().expect("tempdir");
    anthropic_account(tmp.path());
    let cache = tmp.path().join("cloaking-versions.json");
    CliVersions {
        claude: "2.1.200".parse().ok(),
        codex: "0.140.0".parse().ok(),
    }
    .save(&cache)
    .expect("seed cache");
    let upstream = VersionedUpstream::shipping("not-a-version", "nightly");
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());
    upstream.fetched().await;

    let (status, _) = json_response(app, messages_request()).await;
    assert_eq!(status, 200);
    let call = &upstream.inner.calls()[0];
    assert_eq!(call.config.cloaking.cli_version, "2.1.200");
    assert_eq!(
        call.config
            .cloaking
            .codex
            .get("cli-version")
            .map(String::as_str),
        Some("0.140.0")
    );
    let kept = CliVersions::load(&cache);
    assert_eq!(kept.claude, "2.1.200".parse().ok());
    assert_eq!(kept.codex, "0.140.0".parse().ok());
}

/// ARCHITECTURE, "A counter counts outcomes, never attempts": a client that hangs
/// up mid-stream still made a request. The outcome is recorded after the
/// yield loop, so dropping the body skips it and the attempt leaks.
#[tokio::test]
async fn a_client_disconnect_mid_stream_still_reaches_an_outcome() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(
        tmp.path(),
        &TokenData {
            access_token: "anthropic-access".to_string(),
            refresh_token: "anthropic-refresh".to_string(),
            email: "a@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "acct-a".to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        },
    )
    .expect("save token");
    let upstream = Arc::new(FakeUpstream::default());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream);

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "stream": true,
                        "max_tokens": 16,
                        "messages": [{"role": "user", "content": "hi"}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 200);

    // Read one frame, then hang up — the client is gone before the
    // stream completes.
    let mut body = response.into_body().into_data_stream();
    let _first = futures_util::StreamExt::next(&mut body).await;
    drop(body);
    // Let any drop-time accounting run.
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (status, snapshot) = json_response(
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
    let account = &snapshot["providers"]["anthropic"]["accounts"][0];
    let requests = account["totalRequests"].as_i64().expect("requests");
    let ok = account["totalSuccesses"].as_i64().expect("ok");
    let failed = account["totalFailures"].as_i64().expect("failed");
    // Exact counts, not balance: with the `Drop` guard removed nothing is
    // recorded at all, and 0/0/0 balances. The tuple is what fails.
    assert_eq!(
        (requests, ok, failed),
        (1, 0, 1),
        "a disconnected stream did not reach an outcome: \
         {requests} requests, {ok} ok, {failed} failed"
    );
    // The account was serving a 2xx stream when the client hung up: that
    // is not its fault, so it keeps its health and its place in rotation.
    assert_eq!(
        account["available"], true,
        "a client disconnect cooled the account down"
    );
    assert_eq!(
        account["failureCount"], 0,
        "a client disconnect opened a failure streak"
    );
}

/// ARCHITECTURE, "A counter counts outcomes, never attempts", on the request paths
/// rather than on the manager: a 501 refusal counts its attempt as a
/// failed request, and leaves the account's health alone so rotation
/// still offers it.
#[tokio::test]
async fn a_501_refusal_reconciles_and_spares_the_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let upstream = Arc::new(GenericUpstream::default());
    let app = create_app_with_upstream(config_with_groq(tmp.path().to_path_buf()), upstream);

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            // Responses, because Messages is served now: a configured
            // endpoint answers Chat Completions and the relay translates
            // onto it. Responses is the dialect still refused.
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({"model": "groq/llama-3.3-70b", "input": "hi"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 501);

    let (status, snapshot) = json_response(
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
    let account = &snapshot["providers"]["groq"]["accounts"][0];
    let requests = account["totalRequests"].as_i64().expect("requests");
    let ok = account["totalSuccesses"].as_i64().expect("ok");
    let failed = account["totalFailures"].as_i64().expect("failed");
    assert_eq!(
        requests,
        ok + failed,
        "a 501 refusal leaked its attempt: {requests} requests, {ok} ok, {failed} failed"
    );
    // A refusal is not the account's fault: no cooldown, still available.
    assert_eq!(
        account["available"], true,
        "a refusal cooled the account down"
    );
    assert_eq!(
        account["failureCount"], 0,
        "a refusal opened a failure streak"
    );
    // And the daily bucket agrees with the cumulative counters.
    let day = &account["days"][0];
    assert_eq!(
        day["requests"].as_i64().expect("day requests"),
        day["successes"].as_i64().expect("day ok") + day["failures"].as_i64().expect("day failed"),
        "the daily bucket disagrees: {day}"
    );
}

/// An upstream that rejects the body: the most common non-account
/// failure, and the one that used to record no outcome at all.
#[derive(Default)]
struct RejectingUpstream;

impl UpstreamClient for RejectingUpstream {
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::BAD_REQUEST,
                body: json!({"error": {"message": "context length exceeded"}}),
            })
        })
    }

    fn anthropic_messages(&self, _request: UpstreamRequest) -> UpstreamFuture {
        unreachable!("not exercised")
    }
    fn anthropic_messages_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
        unreachable!("not exercised")
    }
    fn anthropic_count_tokens(&self, _request: UpstreamRequest) -> UpstreamFuture {
        unreachable!("not exercised")
    }
    fn codex_responses(&self, _request: UpstreamRequest) -> UpstreamFuture {
        unreachable!("not exercised")
    }
    fn codex_responses_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
        unreachable!("not exercised")
    }
    fn generic_chat_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
        unreachable!("not exercised")
    }
    fn fetch_models(
        &self,
        _kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        unreachable!("not exercised")
    }
    fn fetch_cli_versions(&self) -> CliVersionsFuture {
        unreachable!("not exercised")
    }
}

/// ARCHITECTURE, "A counter counts outcomes, never attempts": an upstream 400 is
/// the client's fault, not the account's. It counts as a failed request
/// so the panels reconcile, and leaves the account in rotation.
#[tokio::test]
async fn an_upstream_400_reconciles_and_spares_the_account() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &groq_key_token()).expect("save groq key");
    let app = create_app_with_upstream(
        config_with_groq(tmp.path().to_path_buf()),
        Arc::new(RejectingUpstream),
    );

    let (status, _) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
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
    assert_eq!(status, 400);

    let (status, snapshot) = json_response(
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
    let account = &snapshot["providers"]["groq"]["accounts"][0];
    let requests = account["totalRequests"].as_i64().expect("requests");
    let ok = account["totalSuccesses"].as_i64().expect("ok");
    let failed = account["totalFailures"].as_i64().expect("failed");
    // Exact counts, not balance: with the refusal call removed nothing is
    // recorded and 0/0/0 balances.
    assert_eq!(
        (requests, ok, failed),
        (1, 0, 1),
        "an upstream 400 did not reach an outcome: \
         {requests} requests, {ok} ok, {failed} failed"
    );
    assert_eq!(
        account["available"], true,
        "a client's bad body cooled the account"
    );
    assert_eq!(
        account["failureCount"], 0,
        "a client's bad body opened a streak"
    );
}

/// AC-6, for real rather than in simulation: six requests in flight at
/// once on one Account, driven concurrently through the router.
///
/// Every other test in this suite calls the recorders in sequence, which
/// is exactly the shape that let a per-account `bool` pass five review
/// rounds while destroying a concurrent success along with its tokens.
/// The counters must show six requests and six successes, and no bucket
/// may disagree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn six_concurrent_requests_each_count_exactly_once() {
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

    let mut inflight = Vec::new();
    for _ in 0..6 {
        let app = app.clone();
        inflight.push(tokio::spawn(async move {
            json_response(
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
                            "messages": [{"role": "user", "content": "hi"}]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
        }));
    }
    for task in inflight {
        let (status, _) = task.await.expect("request panicked");
        assert_eq!(status, 200, "a concurrent request did not succeed");
    }

    let usage: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("anthropic").join("usage.json"))
            .expect("usage file"),
    )
    .expect("usage json");
    let account = usage
        .as_object()
        .expect("accounts")
        .values()
        .next()
        .expect("one account");
    let requests = account["total_requests"].as_i64().expect("requests");
    let ok = account["total_successes"].as_i64().expect("ok");
    let failed = account["total_failures"].as_i64().expect("failed");
    assert_eq!(
        (requests, ok, failed),
        (6, 6, 0),
        "six concurrent requests counted as {requests}/{ok}/{failed}"
    );
    for (date, day) in account["days"].as_object().expect("days") {
        let day_requests = day["requests"].as_i64().expect("day requests");
        let day_ok = day["successes"].as_i64().unwrap_or(0);
        let day_failed = day["failures"].as_i64().unwrap_or(0);
        assert_eq!(
            day_requests,
            day_ok + day_failed,
            "bucket {date} disagrees: {day}"
        );
    }
}

/// AC-7: usage counters key on the name the vendor was asked for, so a
/// `:high` request and a plain request to the same model share one row.
/// Keying on the client's string would split one model's history in two
/// the moment a harness started appending a thinking level.
#[tokio::test]
async fn a_thinking_level_does_not_split_a_models_usage_row() {
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

    for model in ["claude-sonnet-4-6", "claude-sonnet-4-6:high"] {
        let (status, _) = json_response(
            app.clone(),
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer sk-test")
                .header("content-type", "application/json")
                .header("content-length", "1")
                .body(Body::from(
                    json!({"model": model, "messages": [{"role": "user", "content": "hi"}]})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200, "{model} did not succeed");
    }

    // The vendor was asked for the same model both times.
    let asked: Vec<String> = upstream
        .calls()
        .iter()
        .filter_map(|call| {
            call.body
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        asked,
        vec![
            "claude-sonnet-4-6".to_string(),
            "claude-sonnet-4-6".to_string()
        ],
        "the thinking level reached the vendor"
    );

    let usage: Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("anthropic").join("usage.json"))
            .expect("usage file"),
    )
    .expect("usage json");
    let models = usage
        .as_object()
        .expect("accounts")
        .values()
        .next()
        .expect("one account")
        .get("models")
        .and_then(Value::as_object)
        .expect("models");
    let mut names: Vec<&String> = models.keys().collect();
    names.sort();
    assert_eq!(
        names,
        vec!["claude-sonnet-4-6"],
        "the thinking level opened a second usage row: {names:?}"
    );
}

struct GrokUpstream {
    calls: Mutex<Vec<UpstreamRequest>>,
}

impl GrokUpstream {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl UpstreamClient for GrokUpstream {
    fn generic_chat(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("generic chat not used in grok tests")
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("generic stream not used in grok tests")
    }

    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used in grok tests")
    }

    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used in grok tests")
    }

    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in grok tests")
    }

    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in grok tests")
    }

    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in grok tests")
    }

    fn fetch_models(
        &self,
        kind: ProviderKind,
        _account: AvailableAccount,
        _config: Arc<Config>,
    ) -> ModelsFuture {
        assert_eq!(kind, ProviderKind::Grok);
        Box::pin(async { Ok(FetchedModels::new(vec![])) })
    }

    fn grok_chat(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: GrokUpstream::default_json_body(),
            })
        })
    }

    fn grok_chat_stream(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(async {
            Ok(UpstreamSseResponse {
                status: axum::http::StatusCode::OK,
                body: Box::pin(futures_util::stream::iter([
                    Ok(Bytes::from_static(
                        b"data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"thin\"}}]}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"pong\"}}]}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":211,\"completion_tokens\":1,\"total_tokens\":371}}\n\n",
                    )),
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                ])),
            })
        })
    }
}

impl GrokUpstream {
    fn default_json_body() -> Value {
        json!({
            "id": "chatcmpl_grok",
            "object": "chat.completion",
            "created": 1_789_015_110u64,
            "model": "grok-4.6-build",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "pong",
                    "reasoning_content": "the user asked for pong"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 211, "completion_tokens": 1, "total_tokens": 371}
        })
    }
}

fn save_grok_account(dir: &std::path::Path) {
    save_token(
        dir,
        &TokenData {
            access_token: "grok-session-token".to_string(),
            refresh_token: "grok-refresh".to_string(),
            email: "grok@example.com".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            account_uuid: "principal-1".to_string(),
            provider: ProviderId::grok(),
            id_token: None,
            last_refresh_at: None,
            plan_type: Some("tier-3".to_string()),
        },
    )
    .expect("save grok token");
}

#[tokio::test]
async fn grok_pool_serves_chat_completions_and_passes_reasoning_through() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

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
                    "model": "grok-4.6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}],
                    "reasoning_effort": "low"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    // The upstream's reply leaves untouched: content, reasoning_content and all.
    assert_eq!(body["choices"][0]["message"]["content"], "pong");
    assert_eq!(
        body["choices"][0]["message"]["reasoning_content"],
        "the user asked for pong"
    );
    assert_eq!(body["usage"]["prompt_tokens"], 211);

    let calls = upstream.calls.lock().expect("calls lock");
    let request = calls.first().expect("one grok upstream call");
    assert_eq!(request.body["model"], "grok-4.6");
    assert_eq!(request.body["stream"], false);
    assert_eq!(request.body["reasoning_effort"], "low");
    assert_eq!(
        request.body["messages"][0]["content"],
        "reply exactly: pong"
    );
    assert_eq!(request.account.token.access_token, "grok-session-token");
}

#[tokio::test]
async fn grok_messages_route_translates_onto_chat_and_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
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
                    "model": "grok/grok-4.6",
                    "max_tokens": 64,
                    "system": "You are terse.",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    // The reply left in the dialect the client asked in.
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["type"], "thinking");
    assert_eq!(body["content"][0]["thinking"], "the user asked for pong");
    assert_eq!(body["content"][1]["type"], "text");
    assert_eq!(body["content"][1]["text"], "pong");

    let calls = upstream.calls.lock().expect("calls lock");
    let request = calls.first().expect("one grok upstream call");
    // The request left in the chat dialect, with the prefix stripped.
    assert_eq!(request.body["model"], "grok-4.6");
    assert_eq!(request.body["max_tokens"], 64);
    assert_eq!(request.body["messages"][0]["role"], "system");
    assert_eq!(request.body["messages"][0]["content"], "You are terse.");
    assert_eq!(request.body["messages"][1]["role"], "user");
}

#[tokio::test]
async fn grok_chat_stream_passes_chunks_through_with_reasoning_deltas() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, _, body) = raw_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "grok-4.6",
                    "stream": true,
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert!(body.contains("\"reasoning_content\":\"thin\""), "{body}");
    assert!(body.contains("\"content\":\"pong\""), "{body}");
    assert!(body.ends_with("data: [DONE]\n\n"), "{body}");
}

#[tokio::test]
async fn grok_responses_route_translates_onto_chat_and_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
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
                    "model": "grok/grok-4.6",
                    "instructions": "You are terse.",
                    "max_output_tokens": 64,
                    "reasoning": {"effort": "low"},
                    "input": [
                        {"role": "user", "content": [{"type": "input_text", "text": "reply exactly: pong"}]}
                    ]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    // The reply left as a Responses object: reasoning item, then the message.
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["output"][0]["type"], "reasoning");
    assert_eq!(
        body["output"][0]["summary"][0]["text"],
        "the user asked for pong"
    );
    assert_eq!(body["output"][1]["type"], "message");
    assert_eq!(body["output"][1]["content"][0]["type"], "output_text");
    assert_eq!(body["output"][1]["content"][0]["text"], "pong");
    assert_eq!(body["usage"]["input_tokens"], 211);
    assert_eq!(body["usage"]["output_tokens"], 1);

    let calls = upstream.calls.lock().expect("calls lock");
    let request = calls.first().expect("one grok upstream call");
    // The request left in the chat dialect.
    assert_eq!(request.body["model"], "grok-4.6");
    assert_eq!(request.body["max_tokens"], 64);
    assert_eq!(request.body["reasoning_effort"], "low");
    let messages = request.body["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "You are terse.");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "reply exactly: pong");
}

#[tokio::test]
async fn grok_responses_stream_translates_chat_chunks_into_response_events() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    let (status, _, body) = raw_response(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", "Bearer sk-test")
            .header("content-type", "application/json")
            .header("content-length", "1")
            .body(Body::from(
                json!({
                    "model": "grok-4.6",
                    "stream": true,
                    "input": "reply exactly: pong"
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 200);
    assert!(body.contains("\"type\":\"response.created\""), "{body}");
    assert!(
        body.contains("\"type\":\"response.reasoning_text.delta\""),
        "{body}"
    );
    assert!(body.contains("\"delta\":\"thin\""), "{body}");
    assert!(
        body.contains("\"type\":\"response.output_text.delta\""),
        "{body}"
    );
    assert!(body.contains("\"delta\":\"pong\""), "{body}");
    // Items close as they switch; the reasoning item's summary carries its text.
    assert!(body.contains("\"text\":\"thin\""), "{body}");
    assert!(body.contains("\"type\":\"response.completed\""), "{body}");
    assert!(body.contains("\"status\":\"completed\""), "{body}");
    assert!(body.contains("\"input_tokens\":211"), "{body}");
    // The Responses dialect ends at response.completed; no chat [DONE] leaks.
    assert!(!body.contains("[DONE]"), "{body}");
}

#[tokio::test]
async fn admin_reload_picks_up_a_relogged_grok_token_without_a_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    save_grok_account(tmp.path());
    let upstream = Arc::new(GrokUpstream::new());
    let app = create_app_with_upstream(config(tmp.path().to_path_buf()), upstream.clone());

    // A fresh `pengepul login --provider grok` replaces the credential on disk
    // while the relay is running (the token it mints carries scopes the old
    // one lacked).
    let relogged = TokenData {
        access_token: "grok-relogged-token".to_string(),
        refresh_token: "grok-refresh-2".to_string(),
        email: "grok@example.com".to_string(),
        expires_at: "2030-01-01T00:00:00Z".to_string(),
        account_uuid: "principal-1".to_string(),
        provider: ProviderId::grok(),
        id_token: None,
        last_refresh_at: None,
        plan_type: Some("tier-3".to_string()),
    };
    save_token(tmp.path(), &relogged).expect("save relogged token");

    let (status, reloaded) = json_response(
        app.clone(),
        axum::http::Request::builder()
            .method("POST")
            .uri("/admin/reload")
            .header("authorization", "Bearer sk-test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    // The regression: grok was absent from the reload list, so this reported
    // the account as changed nowhere and the relay kept the stale credential.
    assert_eq!(
        reloaded["reloaded"]["grok"]["updated"],
        json!(["grok@example.com"])
    );

    // And the very next request spends the reloaded token, not the old one.
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
                    "model": "grok-4.6",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let calls = upstream.calls.lock().expect("calls lock");
    assert_eq!(
        calls
            .last()
            .expect("one grok call")
            .account
            .token
            .access_token,
        "grok-relogged-token"
    );
}

/// An upstream whose prompt cache lives per Account, the way every real one's does.
///
/// A prefix warmed on one Account reads cold on another, and the split is reported in
/// `prompt_tokens_details.cached_tokens` exactly as the real dialects report it. A test
/// can therefore assert what a turn actually re-billed, rather than asserting which
/// affinity key was computed and trusting that caching follows from it.
#[derive(Default)]
struct PerAccountCacheUpstream {
    /// Accounts whose every request is rejected with 401.
    failing: Mutex<HashSet<String>>,
    /// `(account, prefix)` pairs this upstream has already cached.
    warmed: Mutex<HashSet<(String, String)>>,
    /// One entry per *served* request: account, prompt tokens, cached tokens. A
    /// rejected attempt is not recorded — it served nothing.
    served: Mutex<Vec<(String, u64, u64)>>,
}

/// Every prompt is this long. The number is arbitrary; only its split into cached and
/// re-billed carries meaning, and any real prefix is far above the minimum cacheable
/// unit at every upstream.
const PROMPT_TOKENS: u64 = 4_096;

impl PerAccountCacheUpstream {
    /// The cacheable prefix of a request: its first message, which is the stable
    /// opening both OpenAI-shaped dialects put at the front.
    fn prefix_of(body: &Value) -> String {
        body.get("messages")
            .and_then(|messages| messages.get(0))
            .and_then(|first| first.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn served(&self) -> Vec<(String, u64, u64)> {
        self.served.lock().expect("served lock").clone()
    }
}

impl UpstreamClient for PerAccountCacheUpstream {
    fn generic_chat(
        &self,
        request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        let email = request.account.token.email.clone();
        let prefix = Self::prefix_of(&request.body);

        if self.failing.lock().expect("failing lock").contains(&email) {
            return Box::pin(async move {
                Ok(UpstreamJsonResponse {
                    status: axum::http::StatusCode::UNAUTHORIZED,
                    body: json!({"error": {
                        "message": "invalid api key",
                        "type": "authentication_error"
                    }}),
                })
            });
        }

        // Warm or cold: has *this* Account ever seen *this* prefix?
        let warm = !self
            .warmed
            .lock()
            .expect("warmed lock")
            .insert((email.clone(), prefix));
        let cached_tokens = if warm { PROMPT_TOKENS } else { 0 };
        self.served
            .lock()
            .expect("served lock")
            .push((email, PROMPT_TOKENS, cached_tokens));

        Box::pin(async move {
            Ok(UpstreamJsonResponse {
                status: axum::http::StatusCode::OK,
                body: json!({
                    "id": "chatcmpl_peraccountcache",
                    "object": "chat.completion",
                    "model": "gpt-5.6-sol",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "ok"},
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": PROMPT_TOKENS,
                        "completion_tokens": 1,
                        "total_tokens": PROMPT_TOKENS + 1,
                        "prompt_tokens_details": {"cached_tokens": cached_tokens}
                    }
                }),
            })
        })
    }

    fn generic_chat_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("stream not used in the affinity cache test")
    }
    fn anthropic_messages(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("anthropic not used in the affinity cache test")
    }
    fn anthropic_messages_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("anthropic stream not used in the affinity cache test")
    }
    fn anthropic_count_tokens(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("count_tokens not used in the affinity cache test")
    }
    fn codex_responses(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamJsonResponse>> + Send>> {
        unreachable!("codex not used in the affinity cache test")
    }
    fn codex_responses_stream(
        &self,
        _request: UpstreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<UpstreamSseResponse>> + Send>> {
        unreachable!("codex stream not used in the affinity cache test")
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

/// One conversation's identity as the relay sees it: the `prompt_cache_key` it carries
/// (if any), and the opening of its message list.
type ChatConversation = (Option<&'static str>, &'static str);

/// Which signal tells the relay that two Chat Completions conversations are two
/// conversations. Both must work on their own, and the `BodyKey` half is not
/// hypothetical: measured through the provider's production model configs
/// (`pi-pengepul-provider`, `test/affinity-wire.test.ts`), pi emits
/// `prompt_cache_key` on this dialect when cache retention resolves to long, which is
/// how the operator's own environment is configured (`PI_CACHE_RETENTION=long`). The
/// `DerivedPrefix` half is what covers every client that sends nothing.
#[derive(Clone, Copy)]
enum ChatIdentity {
    /// Distinct `prompt_cache_key`, byte-identical openings.
    BodyKey,
    /// No `prompt_cache_key`, distinct openings.
    DerivedPrefix,
}

impl ChatIdentity {
    fn label(self) -> &'static str {
        match self {
            Self::BodyKey => "prompt_cache_key",
            Self::DerivedPrefix => "derived prefix",
        }
    }

    /// The identity of each conversation. Exactly one of the two fields varies between
    /// them, so the variant proves which signal was load-bearing.
    fn pair(self) -> (ChatConversation, ChatConversation) {
        match self {
            Self::BodyKey => (
                (Some("sess-alpha"), "one shared opening"),
                (Some("sess-bravo"), "one shared opening"),
            ),
            Self::DerivedPrefix => ((None, "alpha opening"), (None, "bravo opening")),
        }
    }
}

/// One turn of a Chat Completions conversation: an opening, then the turn.
///
/// `model` and `tools` are identical in every body off this helper. That is the point:
/// those, plus a `system` field a Chat body does not have, are all the pre-fix affinity
/// key read, so two conversations built this way were one key to it.
fn chat_turn_request(
    cache_key: Option<&str>,
    opening: &str,
    turn: &str,
) -> axum::http::Request<Body> {
    let mut body = json!({
        "model": "commandcode/gpt-5.6-sol",
        "tools": [{"type": "function", "function": {"name": "read_file"}}],
        "messages": [
            {"role": "developer", "content": opening},
            {"role": "user", "content": turn}
        ]
    });
    if let Some(key) = cache_key {
        body["prompt_cache_key"] = json!(key);
    }
    axum::http::Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("authorization", "Bearer sk-test")
        .header("content-type", "application/json")
        .header("content-length", "1024")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Warm two conversations, fail one conversation's account, and check what the *other*
/// conversation's next turn cost.
///
/// The regression this measures: for a Chat Completions body there is no top-level
/// `system`, so the pre-fix key reduced to `{tools, model}` — which every conversation
/// on that model shares. `account_for` then held ONE affinity entry for all of them, so
/// a failure on the account holding it re-pinned all of them at once, and each re-read
/// its prefix cold on an account that had never seen it. Measured on the live relay
/// before the fix: one deepseek conversation split 656/96 across two accounts, and a
/// turn arriving four seconds after the previous one re-billed 17,429 tokens. A
/// four-second gap is not a TTL expiry; it is a different account.
async fn assert_failover_does_not_re_bill_a_sibling(signal: ChatIdentity) {
    let label = signal.label();
    let ((alpha_key, alpha_open), (bravo_key, bravo_open)) = signal.pair();
    let tmp = tempfile::tempdir().expect("tempdir");
    save_token(tmp.path(), &static_key_token("commandcode", "key-90445c90"))
        .expect("save first key");
    save_token(tmp.path(), &static_key_token("commandcode", "key-d792179a"))
        .expect("save second key");
    let upstream = Arc::new(PerAccountCacheUpstream::default());
    let app = create_app_with_upstream(
        config_with_static_provider("commandcode", tmp.path().to_path_buf()),
        upstream.clone(),
    );

    for (key, opening) in [(alpha_key, alpha_open), (bravo_key, bravo_open)] {
        let (status, _) =
            json_response(app.clone(), chat_turn_request(key, opening, "turn one")).await;
        assert_eq!(status, 200, "[{label}] a first turn should succeed");
    }

    let first = upstream.served();
    assert_eq!(
        first.len(),
        2,
        "[{label}] two conversations, two served requests"
    );
    let alpha_account = first[0].0.clone();
    let bravo_account = first[1].0.clone();
    assert_eq!(first[0].2, 0, "[{label}] nothing is cached yet");
    assert_eq!(first[1].2, 0, "[{label}] nothing is cached yet");

    // Alpha's account starts rejecting. Its next turn must move; bravo's must not.
    upstream
        .failing
        .lock()
        .expect("failing lock")
        .insert(alpha_account.clone());

    let (status, _) = json_response(
        app.clone(),
        chat_turn_request(alpha_key, alpha_open, "turn two"),
    )
    .await;
    assert_eq!(status, 200, "[{label}] alpha should fail over, not fail");
    let (status, _) = json_response(
        app.clone(),
        chat_turn_request(bravo_key, bravo_open, "turn two"),
    )
    .await;
    assert_eq!(status, 200, "[{label}] bravo should be unaffected");

    let after = upstream.served();
    assert_eq!(
        after.len(),
        4,
        "[{label}] a rejected attempt serves nothing, so it is not recorded: {after:?}"
    );
    let (alpha_account_two, _, alpha_cached) = after[2].clone();
    let (bravo_account_two, _, bravo_cached) = after[3].clone();

    // The symptom, asserted directly.
    assert_eq!(
        bravo_cached, PROMPT_TOKENS,
        "[{label}] bravo re-billed its prefix ({bravo_cached} cached of {PROMPT_TOKENS}) \
         because another conversation failed over"
    );
    assert_eq!(
        bravo_account_two, bravo_account,
        "[{label}] bravo was moved off its account by alpha's failure"
    );

    // The failover was real, so the assertions above are not vacuous. `alpha_cached`
    // is deliberately not asserted: this upstream is content-addressed like a real
    // prefix cache, so when the two conversations share an opening the destination
    // account already holds it and alpha legitimately re-reads nothing.
    assert_ne!(
        alpha_account_two, alpha_account,
        "[{label}] alpha did not actually move, so this test proves nothing"
    );
    let _ = alpha_cached;

    // And the mechanism: two conversations on one model and tool set are no longer one
    // key, so they spread instead of stacking on whichever account won first.
    assert_ne!(
        alpha_account, bravo_account,
        "[{label}] two conversations on one model and tool set were collapsed onto one account"
    );
}

#[tokio::test]
async fn one_chat_conversation_failing_over_does_not_re_bill_another() {
    assert_failover_does_not_re_bill_a_sibling(ChatIdentity::BodyKey).await;
    assert_failover_does_not_re_bill_a_sibling(ChatIdentity::DerivedPrefix).await;
}
