use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use async_stream::try_stream;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use tower_http::cors::{Any, CorsLayer};

use crate::accounts::{AccountManager, RefreshPolicy, RefreshPolicyKind};
use crate::cloaking_versions::{CliVersions, codex_release, effective, npm_latest};
use crate::config::Config;
use crate::masquerade::{masquerade_request, restore_tool_use_names};
use crate::models::{
    FetchedModels, ModelCatalog, parse_anthropic, parse_codex, parse_openai, upstream_model,
};
use crate::oauth::{refresh_anthropic_tokens, refresh_codex_tokens};
use crate::streaming::{
    AnthropicStreamState, ChatStreamState, ResponsesStreamState, anthropic_sse_to_chat,
    anthropic_sse_to_responses, drain_complete_sse_events, finish_sse_events,
    responses_sse_to_anthropic, responses_sse_to_chat, responses_sse_to_payload, sse,
};
use crate::translate::{
    anthropic_to_openai, anthropic_to_responses, anthropic_to_responses_request,
    chat_to_responses_request, openai_to_anthropic, responses_to_anthropic,
    responses_to_anthropic_message, responses_to_chat_completion,
};
use crate::types::{AvailableAccount, ProviderId, ProviderKind, UsageData};
use crate::upstream::{
    ANTHROPIC_BASE_URL, CODEX_BASE_URL, CODEX_DEFAULT_CLI_VERSION, CODEX_MODELS_PATH,
    CODEX_RESPONSES_PATH, anthropic_headers, apply_cloaking, codex_headers, generic_base_url,
    generic_chat_headers, normalize_codex_responses_body,
};
use crate::utils::now_iso;

const RATE_LIMIT_WINDOW: Duration = Duration::from_mins(1);
const RATE_LIMIT_MAX: u32 = 60;

pub type UpstreamFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<UpstreamJsonResponse>> + Send>>;
pub type UpstreamSseStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;
pub type UpstreamSseFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<UpstreamSseResponse>> + Send>>;
pub type ModelsFuture = Pin<Box<dyn Future<Output = anyhow::Result<FetchedModels>> + Send>>;
pub type CliVersionsFuture = Pin<Box<dyn Future<Output = anyhow::Result<CliVersions>> + Send>>;

#[derive(Debug, Clone)]
pub struct UpstreamRequest {
    pub body: Value,
    pub request_headers: BTreeMap<String, String>,
    pub account: AvailableAccount,
    pub config: Arc<Config>,
}

#[derive(Debug, Clone)]
pub struct UpstreamJsonResponse {
    pub status: StatusCode,
    pub body: Value,
}

pub struct UpstreamSseResponse {
    pub status: StatusCode,
    pub body: UpstreamSseStream,
}

pub trait UpstreamClient: Send + Sync {
    fn anthropic_messages(&self, request: UpstreamRequest) -> UpstreamFuture;
    fn anthropic_messages_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture;
    fn anthropic_count_tokens(&self, request: UpstreamRequest) -> UpstreamFuture;
    fn codex_responses(&self, request: UpstreamRequest) -> UpstreamFuture;
    fn codex_responses_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture;
    /// A configured OpenAI-compatible endpoint's Chat Completions call. The
    /// request carries the resolved base URL in its config; the body is a plain
    /// chat completion.
    fn generic_chat(&self, request: UpstreamRequest) -> UpstreamFuture;
    fn generic_chat_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture;
    fn fetch_models(
        &self,
        kind: ProviderKind,
        account: AvailableAccount,
        config: Arc<Config>,
    ) -> ModelsFuture;
    /// The CLI versions the vendors currently ship, for Cloaking to present. A client
    /// with no version source reports an error; the relay then keeps what it has.
    fn fetch_cli_versions(&self) -> CliVersionsFuture {
        Box::pin(async { anyhow::bail!("no cli version source") })
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    /// What the vendors are known to ship, and `config` with those versions applied;
    /// the latter is what every upstream call sees. Replaced whole when a fetch moves
    /// a version.
    cloaking: Arc<StdRwLock<Cloaking>>,
    body_limit: BodyLimit,
    upstream: Arc<dyn UpstreamClient>,
    account_managers: Arc<AccountManagers>,
    rate_limit_buckets: Arc<StdMutex<BTreeMap<String, RateLimitBucket>>>,
    catalog: Arc<StdRwLock<ModelCatalog>>,
}

struct AccountManagers {
    anthropic: tokio::sync::Mutex<AccountManager>,
    codex: tokio::sync::Mutex<AccountManager>,
    generic: BTreeMap<String, tokio::sync::Mutex<AccountManager>>,
}

#[derive(Clone)]
struct StreamAccounting {
    state: AppState,
    provider: ProviderId,
    account: AvailableAccount,
    /// The upstream model name: what per-model counters are keyed by.
    model: String,
}

#[derive(Debug, Clone)]
enum BodyLimit {
    Unlimited,
    Limited(u64),
    Invalid,
}

#[derive(Debug, Clone)]
struct RateLimitBucket {
    count: u32,
    reset_at: Instant,
}

#[derive(Debug, Clone)]
struct AppError {
    status: StatusCode,
    message: String,
    error_type: Option<&'static str>,
    provider: Option<ProviderId>,
}

impl AppError {
    fn simple(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            error_type: None,
            provider: None,
        }
    }

    fn provider(
        status: StatusCode,
        message: impl Into<String>,
        error_type: &'static str,
        provider: ProviderId,
    ) -> Self {
        Self {
            status,
            message: message.into(),
            error_type: Some(error_type),
            provider: Some(provider),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut error = serde_json::Map::new();
        error.insert("message".to_string(), Value::String(self.message));
        if let Some(error_type) = self.error_type {
            error.insert("type".to_string(), Value::String(error_type.to_string()));
        }
        if let Some(provider) = self.provider {
            error.insert("provider".to_string(), Value::String(provider.to_string()));
        }
        (self.status, Json(json!({"error": error}))).into_response()
    }
}

#[derive(Debug, Clone, Default)]
struct HttpUpstreamClient {
    client: reqwest::Client,
}

impl UpstreamClient for HttpUpstreamClient {
    fn fetch_cli_versions(&self) -> CliVersionsFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let npm = send_get(
                client.clone(),
                CLAUDE_CLI_REGISTRY_URL.to_string(),
                BTreeMap::new(),
                CLI_VERSIONS_TIMEOUT_MS,
            )
            .await?;
            let github = send_get(
                client,
                CODEX_CLI_RELEASE_URL.to_string(),
                BTreeMap::from([
                    ("User-Agent".to_string(), "pengepul".to_string()),
                    (
                        "Accept".to_string(),
                        "application/vnd.github+json".to_string(),
                    ),
                ]),
                CLI_VERSIONS_TIMEOUT_MS,
            )
            .await?;
            Ok(CliVersions {
                claude: npm_latest(&npm),
                codex: codex_release(&github),
            })
        })
    }

    fn anthropic_messages(&self, request: UpstreamRequest) -> UpstreamFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let stream = request
                .body
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let timeout_ms = if stream {
                request.config.timeouts.stream_messages_ms
            } else {
                request.config.timeouts.messages_ms
            };
            let model = request
                .body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("claude-sonnet-4-6");
            let body = apply_cloaking(
                &request.body,
                &request.request_headers,
                &request.account,
                &request.config,
            );
            let headers = anthropic_headers(
                &request.account.token.access_token,
                stream,
                timeout_ms,
                model,
                &request.config,
                &request.request_headers,
                false,
            );
            send_json(
                client,
                format!("{ANTHROPIC_BASE_URL}/v1/messages?beta=true"),
                headers,
                body,
                timeout_ms,
            )
            .await
        })
    }

    fn anthropic_messages_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let timeout_ms = request.config.timeouts.stream_messages_ms;
            let model = request
                .body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("claude-sonnet-4-6");
            let body = apply_cloaking(
                &request.body,
                &request.request_headers,
                &request.account,
                &request.config,
            );
            let headers = anthropic_headers(
                &request.account.token.access_token,
                true,
                timeout_ms,
                model,
                &request.config,
                &request.request_headers,
                false,
            );
            send_stream(
                client,
                format!("{ANTHROPIC_BASE_URL}/v1/messages?beta=true"),
                headers,
                body,
                timeout_ms,
            )
            .await
        })
    }

    fn anthropic_count_tokens(&self, request: UpstreamRequest) -> UpstreamFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let model = request
                .body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("claude-sonnet-4-6");
            let headers = anthropic_headers(
                &request.account.token.access_token,
                false,
                request.config.timeouts.count_tokens_ms,
                model,
                &request.config,
                &request.request_headers,
                false,
            );
            send_json(
                client,
                format!("{ANTHROPIC_BASE_URL}/v1/messages/count_tokens?beta=true"),
                headers,
                request.body,
                request.config.timeouts.count_tokens_ms,
            )
            .await
        })
    }

    fn codex_responses(&self, request: UpstreamRequest) -> UpstreamFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let stream = request
                .body
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let timeout_ms = if stream {
                request.config.timeouts.stream_messages_ms
            } else {
                request.config.timeouts.messages_ms
            };
            send_json(
                client,
                format!("{CODEX_BASE_URL}{CODEX_RESPONSES_PATH}"),
                codex_headers(&request.account, stream, &request.config),
                request.body,
                timeout_ms,
            )
            .await
        })
    }

    fn codex_responses_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        let client = self.client.clone();
        Box::pin(async move {
            send_stream(
                client,
                format!("{CODEX_BASE_URL}{CODEX_RESPONSES_PATH}"),
                codex_headers(&request.account, true, &request.config),
                request.body,
                request.config.timeouts.stream_messages_ms,
            )
            .await
        })
    }

    fn generic_chat(&self, request: UpstreamRequest) -> UpstreamFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let stream = request
                .body
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let timeout_ms = if stream {
                request.config.timeouts.stream_messages_ms
            } else {
                request.config.timeouts.messages_ms
            };
            let base_url = generic_base_url(&request.config, &request.account.provider.id)
                .context("generic provider missing from config")?;
            send_json(
                client,
                format!("{base_url}/chat/completions"),
                generic_chat_headers(&request.account),
                request.body,
                timeout_ms,
            )
            .await
        })
    }

    fn generic_chat_stream(&self, request: UpstreamRequest) -> UpstreamSseFuture {
        let client = self.client.clone();
        Box::pin(async move {
            let base_url = generic_base_url(&request.config, &request.account.provider.id)
                .context("generic provider missing from config")?;
            send_stream(
                client,
                format!("{base_url}/chat/completions"),
                generic_chat_headers(&request.account),
                request.body,
                request.config.timeouts.stream_messages_ms,
            )
            .await
        })
    }

    fn fetch_models(
        &self,
        kind: ProviderKind,
        account: AvailableAccount,
        config: Arc<Config>,
    ) -> ModelsFuture {
        let client = self.client.clone();
        let timeout = config.timeouts.count_tokens_ms;
        Box::pin(async move {
            match kind {
                ProviderKind::Generic => {
                    let base_url = generic_base_url(&config, &account.provider.id)
                        .context("generic provider missing from config")?;
                    let headers = generic_chat_headers(&account);
                    let body =
                        send_get(client, format!("{base_url}/models"), headers, timeout).await?;
                    Ok(parse_openai(&body))
                }
                ProviderKind::Anthropic => {
                    let headers = BTreeMap::from([
                        (
                            "authorization".to_string(),
                            format!("Bearer {}", account.token.access_token),
                        ),
                        ("anthropic-version".to_string(), "2023-06-01".to_string()),
                        ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
                    ]);
                    let body = send_get(
                        client,
                        format!("{ANTHROPIC_BASE_URL}/v1/models"),
                        headers,
                        timeout,
                    )
                    .await?;
                    Ok(parse_anthropic(&body))
                }
                ProviderKind::Codex => {
                    let version = config
                        .cloaking
                        .codex
                        .get("cli-version")
                        .map_or(CODEX_DEFAULT_CLI_VERSION, String::as_str);
                    let url =
                        format!("{CODEX_BASE_URL}{CODEX_MODELS_PATH}?client_version={version}");
                    let headers = codex_headers(&account, false, &config);
                    let body = send_get(client, url, headers, timeout).await?;
                    Ok(parse_codex(&body))
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum RequestRoute {
    Chat,
    Responses,
    Messages,
}

pub fn create_app(config: Config) -> Router {
    create_app_with_upstream(config, Arc::new(HttpUpstreamClient::default()))
}

pub fn create_app_with_upstream(config: Config, upstream: Arc<dyn UpstreamClient>) -> Router {
    if let Err(error) = crate::tokens::migrate_legacy_layout(&config.auth_dir) {
        tracing::warn!(?error, "legacy token layout migration failed");
    }
    let body_limit = parse_body_limit(&config.body_limit);
    let account_managers = build_account_managers(&config);
    let config = Arc::new(config);
    let cloaking = Arc::new(StdRwLock::new(Cloaking::new(
        &config,
        CliVersions::load(&cli_versions_path(&config)),
    )));
    let state = AppState {
        config,
        cloaking,
        body_limit,
        upstream,
        account_managers: Arc::new(account_managers),
        rate_limit_buckets: Arc::new(StdMutex::new(BTreeMap::new())),
        catalog: Arc::new(StdRwLock::new(ModelCatalog::default())),
    };

    // Keep the model catalog fresh off the request path. Requires a Tokio runtime, which
    // both `serve` (create_app runs inside block_on) and the tests (#[tokio::test]) provide.
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(model_catalog_refresh_loop(state.clone()));
        tokio::spawn(cli_versions_refresh_loop(state.clone()));
    }

    // The two SDK families disagree on where `/v1` lives: OpenAI clients want it in the
    // base URL and append `/chat/completions`; Anthropic clients want it absent and append
    // `/v1/messages`. A base URL of `http://host:port/v1` therefore reaches Anthropic
    // routes as `/v1/v1/messages`. Mounting the API at both prefixes lets one documented
    // base URL serve every client. Exactly one duplicate is tolerated; `/v1/v1/v1` is 404.
    let api = Router::new()
        .route("/models", get(models))
        .route("/chat/completions", post(chat_completions))
        .route("/responses", post(responses))
        .route("/messages", post(messages))
        .route("/messages/count_tokens", post(count_tokens));
    Router::new()
        .route("/health", get(health))
        .route("/admin/accounts", get(admin_accounts))
        .route("/admin/reload", post(admin_reload))
        .nest("/v1", api.clone())
        .nest("/v1/v1", api)
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers(Any),
        )
}

fn build_account_managers(config: &Config) -> AccountManagers {
    let mut anthropic = AccountManager::new(
        config.auth_dir.clone(),
        ProviderId::anthropic(),
        |refresh_token| Box::pin(refresh_anthropic_tokens(refresh_token)),
        RefreshPolicy::default(),
    );
    let mut codex = AccountManager::new(
        config.auth_dir.clone(),
        ProviderId::codex(),
        |refresh_token| Box::pin(refresh_codex_tokens(refresh_token)),
        RefreshPolicy {
            kind: RefreshPolicyKind::SinceLastRefresh,
            seconds: 8 * 24 * 60 * 60,
        },
    );
    let _ = anthropic.load();
    let _ = codex.load();
    // One manager per configured provider; static keys never refresh, so the
    // callback exists only to satisfy the type and must never be called.
    let mut generic = BTreeMap::new();
    for id in config.providers.keys() {
        let mut manager = AccountManager::new(
            config.auth_dir.clone(),
            ProviderId::generic(id.clone()),
            |_refresh_token| Box::pin(async { anyhow::bail!("static keys do not refresh") }),
            RefreshPolicy {
                kind: RefreshPolicyKind::Never,
                seconds: 0,
            },
        );
        let _ = manager.load();
        generic.insert(id.clone(), tokio::sync::Mutex::new(manager));
    }
    tracing::info!(
        anthropic = anthropic.account_count(),
        codex = codex.account_count(),
        generic = generic.len(),
        "loaded provider accounts"
    );
    AccountManagers {
        anthropic: tokio::sync::Mutex::new(anthropic),
        codex: tokio::sync::Mutex::new(codex),
        generic,
    }
}

const MODEL_CATALOG_TTL: Duration = Duration::from_mins(15);

const CLAUDE_CLI_REGISTRY_URL: &str = "https://registry.npmjs.org/@anthropic-ai/claude-code";
const CODEX_CLI_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CLI_VERSIONS_TIMEOUT_MS: u64 = 30_000;
const CLI_VERSIONS_TTL: Duration = Duration::from_hours(24);
const CLI_VERSIONS_RETRY: Duration = Duration::from_hours(1);

fn cli_versions_path(config: &Config) -> std::path::PathBuf {
    config.auth_dir.join("cloaking-versions.json")
}

struct Cloaking {
    known: CliVersions,
    config: Arc<Config>,
}

impl Cloaking {
    fn new(config: &Config, known: CliVersions) -> Self {
        Self {
            config: cloak_versions(config, &known),
            known,
        }
    }
}

/// `config` with Cloaking's versions raised to the fetched ones where those are newer.
fn cloak_versions(config: &Config, fetched: &CliVersions) -> Arc<Config> {
    let mut cloaked = config.clone();
    cloaked.cloaking.cli_version = effective(&config.cloaking.cli_version, fetched.claude.as_ref());
    let codex_floor = config
        .cloaking
        .codex
        .get("cli-version")
        .map_or(CODEX_DEFAULT_CLI_VERSION, String::as_str);
    cloaked.cloaking.codex.insert(
        "cli-version".to_string(),
        effective(codex_floor, fetched.codex.as_ref()),
    );
    Arc::new(cloaked)
}

/// The config every upstream call is built from.
fn cloaked_config(state: &AppState) -> Arc<Config> {
    state
        .cloaking
        .read()
        .expect("cloaking lock poisoned")
        .config
        .clone()
}

/// Keep Cloaking's versions at what the vendors ship. The first fetch runs as soon as
/// the relay is up; a success sleeps a day, a failure retries within the hour.
async fn cli_versions_refresh_loop(state: AppState) {
    loop {
        let interval = if refresh_cli_versions(&state).await {
            CLI_VERSIONS_TTL
        } else {
            CLI_VERSIONS_RETRY
        };
        tokio::time::sleep(interval).await;
    }
}

async fn refresh_cli_versions(state: &AppState) -> bool {
    let fetched = match state.upstream.fetch_cli_versions().await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(?error, "cli version fetch failed");
            return false;
        }
    };
    // A field the registry did not yield keeps its last known value: a bad body must
    // never move a version backwards or blank the cache.
    let (known, changed) = {
        let mut cloaking = state.cloaking.write().expect("cloaking lock poisoned");
        let known = CliVersions {
            claude: fetched.claude.or(cloaking.known.claude),
            codex: fetched.codex.or(cloaking.known.codex),
        };
        let next = Cloaking::new(&state.config, known.clone());
        let changed = next.config.cloaking != cloaking.config.cloaking;
        *cloaking = next;
        (known, changed)
    };
    if changed {
        let config = cloaked_config(state);
        tracing::info!(
            claude = %config.cloaking.cli_version,
            codex = %config.cloaking.codex.get("cli-version").map_or("", String::as_str),
            "cloaking versions updated"
        );
    }
    if let Err(error) = known.save(&cli_versions_path(&state.config)) {
        tracing::warn!(?error, "cli version cache write failed");
    }
    true
}

async fn model_catalog_refresh_loop(state: AppState) {
    loop {
        refresh_model_catalog(&state).await;
        tokio::time::sleep(MODEL_CATALOG_TTL).await;
    }
}

async fn refresh_model_catalog(state: &AppState) {
    for kind in [ProviderKind::Anthropic, ProviderKind::Codex] {
        let Some(account) = catalog_account(state, kind).await else {
            continue;
        };
        match state
            .upstream
            .fetch_models(kind, account, cloaked_config(state))
            .await
        {
            Ok(FetchedModels { ids, metadata }) => {
                state
                    .catalog
                    .write()
                    .expect("catalog lock poisoned")
                    .set_direct(kind, FetchedModels::with_metadata(ids, metadata));
            }
            Err(error) => {
                tracing::warn!(
                    provider = kind.canonical_id(),
                    ?error,
                    "model list fetch failed"
                );
            }
        }
    }
    // Configured providers: one fetch each, advertised under their own prefix.
    for provider_id in state.config.providers.keys() {
        let provider = ProviderId::generic(provider_id.clone());
        let Some(account) = catalog_account_id(state, &provider).await else {
            continue;
        };
        match state
            .upstream
            .fetch_models(ProviderKind::Generic, account, cloaked_config(state))
            .await
        {
            Ok(FetchedModels { ids, metadata }) => {
                state
                    .catalog
                    .write()
                    .expect("catalog lock poisoned")
                    .set_generic(provider_id, FetchedModels::with_metadata(ids, metadata));
            }
            Err(error) => {
                tracing::warn!(provider = provider_id, ?error, "model list fetch failed");
            }
        }
    }
}

/// An account for a background model fetch: refresh its token if due, then hand it back. The
/// manager lock drops before the caller fetches, so the request path is never blocked on the
/// model-list I/O.
async fn catalog_account(state: &AppState, kind: ProviderKind) -> Option<AvailableAccount> {
    match kind {
        ProviderKind::Anthropic => {
            let mut manager = state.account_managers.anthropic.lock().await;
            let email = manager.next_account()?.token.email;
            let _ = manager.refresh_if_due(&email).await;
            manager.account(&email)
        }
        ProviderKind::Codex => {
            let mut manager = state.account_managers.codex.lock().await;
            let email = manager.next_account()?.token.email;
            let _ = manager.refresh_if_due(&email).await;
            manager.account(&email)
        }
        ProviderKind::Generic => None,
    }
}

/// An account for a configured provider's model fetch: static keys never refresh.
async fn catalog_account_id(state: &AppState, provider: &ProviderId) -> Option<AvailableAccount> {
    let manager = state.account_managers.generic.get(provider.id.as_ref())?;
    let mut manager = manager.lock().await;
    manager.next_account()
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn admin_accounts(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(error) = require_api_key(&state, &headers, false) {
        return error.into_response();
    }

    let anthropic = state.account_managers.anthropic.lock().await;
    let codex = state.account_managers.codex.lock().await;
    let mut providers = serde_json::Map::from_iter([
        (
            ProviderId::anthropic().to_string(),
            json!({
                "accounts": anthropic.snapshots(),
                "account_count": anthropic.account_count()
            }),
        ),
        (
            ProviderId::codex().to_string(),
            json!({
                "accounts": codex.snapshots(),
                "account_count": codex.account_count()
            }),
        ),
    ]);
    drop(anthropic);
    drop(codex);
    for (id, manager) in &state.account_managers.generic {
        let manager = manager.lock().await;
        providers.insert(
            id.clone(),
            json!({
                "accounts": manager.snapshots(),
                "account_count": manager.account_count()
            }),
        );
    }

    Json(json!({"providers": providers, "generated_at": now_iso()})).into_response()
}

async fn admin_reload(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(error) = require_api_key(&state, &headers, false) {
        return error.into_response();
    }

    let anthropic = state.account_managers.anthropic.lock().await.reload();
    let codex = state.account_managers.codex.lock().await.reload();
    let mut generic = BTreeMap::new();
    let mut generic_failed = None;
    for (id, manager) in &state.account_managers.generic {
        match manager.lock().await.reload() {
            Ok(result) => {
                generic.insert(id.clone(), result);
            }
            Err(error) => generic_failed = Some(error),
        }
    }
    if let Some(error) = generic_failed {
        return AppError::simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to reload accounts: {error}"),
        )
        .into_response();
    }
    let reloaded = match (anthropic, codex) {
        (Ok(anthropic), Ok(codex)) => {
            let mut map = serde_json::Map::from_iter([
                (ProviderId::anthropic().to_string(), anthropic),
                (ProviderId::codex().to_string(), codex),
            ]);
            map.extend(generic);
            map
        }
        (Err(error), _) | (_, Err(error)) => {
            return AppError::simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to reload accounts: {error}"),
            )
            .into_response();
        }
    };

    // Reloading accounts can add a provider that now has credentials to fetch models with.
    let refresh_state = state.clone();
    tokio::spawn(async move { refresh_model_catalog(&refresh_state).await });

    Json(json!({"reloaded": reloaded, "generated_at": now_iso()})).into_response()
}

async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(error) = require_api_key(&state, &headers, true) {
        return error.into_response();
    }

    let created = chrono::Utc::now().timestamp();
    let models = state
        .catalog
        .read()
        .expect("catalog lock poisoned")
        .advertised()
        .into_iter()
        .map(|model| {
            let mut entry = json!({
                "id": model.id,
                "object": "model",
                "created": created,
                "owned_by": model.provider.id.as_ref()
            });
            // Metadata is additive: known fields merge in, unknown ones stay omitted, and a
            // client reading only `id` sees the same shape as before.
            if let Some(metadata) = model.metadata.and_then(|m| serde_json::to_value(m).ok())
                && let (Some(entry), Some(extra)) = (entry.as_object_mut(), metadata.as_object())
            {
                entry.extend(extra.clone());
            }
            entry
        })
        .collect::<Vec<_>>();

    Json(json!({"object": "list", "data": models})).into_response()
}

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body = match parse_request(&state, &headers, &body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if !non_empty_array(body.get("messages")) {
        return AppError::simple(
            StatusCode::BAD_REQUEST,
            "messages is required and must be a non-empty array",
        )
        .into_response();
    }
    route_provider_request(&state, &headers, &body, RequestRoute::Chat).await
}

async fn responses(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let body = match parse_request(&state, &headers, &body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if body.get("input").is_none() && body.get("messages").is_none() {
        return AppError::simple(StatusCode::BAD_REQUEST, "input is required").into_response();
    }
    route_provider_request(&state, &headers, &body, RequestRoute::Responses).await
}

async fn messages(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let body = match parse_request(&state, &headers, &body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if !non_empty_array(body.get("messages")) {
        return AppError::simple(
            StatusCode::BAD_REQUEST,
            "messages is required and must be a non-empty array",
        )
        .into_response();
    }
    route_provider_request(&state, &headers, &body, RequestRoute::Messages).await
}

async fn count_tokens(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let body = match parse_request(&state, &headers, &body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let Some(model_id) = required_model(&body) else {
        return AppError::simple(StatusCode::BAD_REQUEST, "model is required").into_response();
    };
    let Some(provider) = state
        .catalog
        .read()
        .expect("catalog lock poisoned")
        .resolve_id(model_id, &state.config.providers)
    else {
        return AppError::simple(
            StatusCode::BAD_REQUEST,
            format!("unknown model: {model_id}"),
        )
        .into_response();
    };
    let model = upstream_model(model_id, &provider).to_string();
    if provider.kind != ProviderKind::Anthropic {
        return AppError::provider(
            StatusCode::NOT_IMPLEMENTED,
            format!("count_tokens is not supported for the {provider} provider"),
            "unsupported_endpoint_for_provider",
            provider.clone(),
        )
        .into_response();
    }
    let account = match next_provider_account(&state, provider.clone()).await {
        Ok(account) => account,
        Err(error) => return error.into_response(),
    };
    let body = body_with_model(&body, &model);
    match state
        .upstream
        .anthropic_count_tokens(UpstreamRequest {
            body,
            request_headers: headers_to_map(&headers),
            account: account.clone(),
            config: cloaked_config(&state),
        })
        .await
    {
        Ok(response) => {
            if response.status.is_success() {
                // Token counting bills no usage; the success still clears
                // the account's failure streak.
                record_provider_success(&state, provider.clone(), &account, None, &model).await;
            } else {
                record_provider_failure(&state, provider.clone(), &account, response.status, None)
                    .await;
            }
            (response.status, Json(response.body)).into_response()
        }
        Err(error) => {
            record_provider_failure(
                &state,
                provider.clone(),
                &account,
                StatusCode::BAD_GATEWAY,
                Some(&error.to_string()),
            )
            .await;
            upstream_error_response(provider, &error)
        }
    }
}

fn parse_request(state: &AppState, headers: &HeaderMap, body: &[u8]) -> Result<Value, AppError> {
    require_api_key(state, headers, true)?;
    enforce_body_limit(state, headers)?;
    serde_json::from_slice(body)
        .map_err(|_| AppError::simple(StatusCode::BAD_REQUEST, "invalid JSON body"))
}

async fn route_provider_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    route: RequestRoute,
) -> Response {
    let Some(model_id) = required_model(body) else {
        return AppError::simple(StatusCode::BAD_REQUEST, "model is required").into_response();
    };
    let Some(provider) = state
        .catalog
        .read()
        .expect("catalog lock poisoned")
        .resolve_id(model_id, &state.config.providers)
    else {
        return AppError::simple(
            StatusCode::BAD_REQUEST,
            format!("unknown model: {model_id}"),
        )
        .into_response();
    };
    let model = upstream_model(model_id, &provider).to_string();
    let client_wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let attempts = provider_account_count(state, provider.clone()).await.max(1);
    let mut last_response = None;

    for _ in 0..attempts {
        let account = match next_provider_account(state, provider.clone()).await {
            Ok(account) => account,
            Err(error) if error.error_type == Some("token_refresh_failed") => {
                last_response = Some(error.into_response());
                continue;
            }
            Err(error) => return last_response.unwrap_or_else(|| error.into_response()),
        };
        let mut response = match provider.kind {
            ProviderKind::Generic => {
                if matches!(route, RequestRoute::Chat) {
                    route_generic_chat_request(
                        state,
                        headers,
                        body,
                        &model,
                        &account,
                        client_wants_stream,
                    )
                    .await
                } else {
                    return AppError::provider(
                        StatusCode::NOT_IMPLEMENTED,
                        format!(
                            "the {} dialect is not supported for the {provider} provider",
                            route_name(route)
                        ),
                        "unsupported_endpoint_for_provider",
                        provider,
                    )
                    .into_response();
                }
            }
            ProviderKind::Codex => {
                route_codex_request(
                    state,
                    headers,
                    body,
                    route,
                    &model,
                    &account,
                    client_wants_stream,
                )
                .await
            }
            ProviderKind::Anthropic => {
                route_anthropic_request(
                    state,
                    headers,
                    body,
                    route,
                    &model,
                    &account,
                    client_wants_stream,
                )
                .await
            }
        };
        if !should_retry_upstream_status(response.status())
            && !is_account_scoped_billing_failure(state, provider.clone(), &account, &mut response)
                .await
        {
            return response;
        }
        // an account-scoped billing failure retries: a sibling account may still have credits
        last_response = Some(response);
    }

    last_response.unwrap_or_else(|| {
        AppError::provider(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("no available {provider} account"),
            "no_account_for_provider",
            provider,
        )
        .into_response()
    })
}

async fn provider_account_count(state: &AppState, provider: ProviderId) -> usize {
    match provider.kind {
        ProviderKind::Anthropic => state
            .account_managers
            .anthropic
            .lock()
            .await
            .account_count(),
        ProviderKind::Codex => state.account_managers.codex.lock().await.account_count(),
        ProviderKind::Generic => {
            let Some(manager) = state.account_managers.generic.get(provider.id.as_ref()) else {
                return 0;
            };
            manager.lock().await.account_count()
        }
    }
}

fn should_retry_upstream_status(status: StatusCode) -> bool {
    // 501 is pengepul's own "unsupported route for provider" response, not a transient
    // upstream failure; retrying it would only re-generate the same error each pass.
    matches!(status.as_u16(), 401 | 403 | 429 | 500 | 502..=599)
}

/// Error strings that mark an upstream rejection as about the account's credit or quota
/// rather than the request itself. Credits live per account, so a sibling account may
/// still serve the request and it should fail over instead of failing the client.
const BILLING_FAILURE_MARKERS: [&str; 4] = [
    "insufficient credits",
    "insufficient_quota",
    "insufficient quota",
    "out of credits",
];

/// Whether a non-retryable-by-status upstream rejection is account-scoped billing —
/// credits live per account, so a sibling account may still serve the request. Records
/// the failure as billing when it matches. The response is buffered for the check and
/// rebuilt in place either way.
async fn is_account_scoped_billing_failure(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    response: &mut Response,
) -> bool {
    let status = response.status();
    if status != StatusCode::BAD_REQUEST && status != StatusCode::PAYMENT_REQUIRED {
        return false;
    }
    record_billing_scoped_failure(state, provider, account, response).await
}

/// Buffer a 400/402 response and decide whether the upstream rejected this account's
/// balance rather than the request. When it matches, the failure is recorded as billing
/// (a long cooldown — credits do not return mid-session) and the request may fail over.
/// Returns the matched flag; the response is rebuilt in place either way.
async fn record_billing_scoped_failure(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    response: &mut Response,
) -> bool {
    let (parts, body) = std::mem::take(response).into_parts();
    let Some(buffered) = axum::body::to_bytes(body, 64 * 1024).await.ok() else {
        *response = Response::from_parts(parts, Body::empty());
        return false;
    };
    let text = String::from_utf8_lossy(&buffered).to_ascii_lowercase();
    let billing = BILLING_FAILURE_MARKERS
        .iter()
        .any(|marker| text.contains(marker));
    if billing {
        record_provider_failure_kind(
            state,
            provider,
            account,
            "billing",
            Some(&String::from_utf8_lossy(&buffered)),
        )
        .await;
    }
    *response = Response::from_parts(parts, Body::from(buffered));
    billing
}

/// The route's display name for error messages ("Chat Completions" etc.).
fn route_name(route: RequestRoute) -> &'static str {
    match route {
        RequestRoute::Chat => "Chat Completions",
        RequestRoute::Responses => "Responses",
        RequestRoute::Messages => "Messages",
    }
}

/// Serve a Chat Completions request on a configured OpenAI-compatible endpoint.
/// The body goes upstream untouched (the endpoint speaks chat natively), stream
/// flag rewritten to what the client asked for.
async fn route_generic_chat_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    model: &str,
    account: &AvailableAccount,
    client_wants_stream: bool,
) -> Response {
    let mut upstream_body = body_with_model(body, model);
    if let Some(object) = upstream_body.as_object_mut() {
        object.insert("stream".to_string(), Value::Bool(client_wants_stream));
    }
    if client_wants_stream {
        return match state
            .upstream
            .generic_chat_stream(UpstreamRequest {
                body: upstream_body,
                request_headers: headers_to_map(headers),
                account: account.clone(),
                config: cloaked_config(state),
            })
            .await
        {
            Ok(response) => {
                let accounting = stream_accounting(
                    state,
                    account.provider.clone(),
                    account,
                    response.status,
                    model,
                )
                .await;
                sse_upstream_response(
                    response,
                    account.provider.clone(),
                    RequestRoute::Chat,
                    model,
                    accounting,
                    Arc::new(BTreeMap::new()),
                )
            }
            Err(error) => {
                upstream_failure_response(state, account.provider.clone(), account, &error).await
            }
        };
    }
    match state
        .upstream
        .generic_chat(UpstreamRequest {
            body: upstream_body,
            request_headers: headers_to_map(headers),
            account: account.clone(),
            config: cloaked_config(state),
        })
        .await
    {
        Ok(response) => {
            record_json_result(state, account.provider.clone(), account, &response, model).await;
            json_upstream_response(
                response,
                &account.provider,
                RequestRoute::Chat,
                model,
                &BTreeMap::new(),
            )
        }
        Err(error) => {
            upstream_failure_response(state, account.provider.clone(), account, &error).await
        }
    }
}

async fn route_codex_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    route: RequestRoute,
    model: &str,
    account: &AvailableAccount,
    client_wants_stream: bool,
) -> Response {
    let body = codex_request_body(body, model, route);
    if client_wants_stream {
        return match state
            .upstream
            .codex_responses_stream(UpstreamRequest {
                body,
                request_headers: headers_to_map(headers),
                account: account.clone(),
                config: cloaked_config(state),
            })
            .await
        {
            Ok(response) => {
                let accounting = stream_accounting(
                    state,
                    account.provider.clone(),
                    account,
                    response.status,
                    model,
                )
                .await;
                sse_upstream_response(
                    response,
                    account.provider.clone(),
                    route,
                    model,
                    accounting,
                    Arc::new(BTreeMap::new()),
                )
            }
            Err(error) => {
                upstream_failure_response(state, account.provider.clone(), account, &error).await
            }
        };
    }
    match state
        .upstream
        .codex_responses(UpstreamRequest {
            body,
            request_headers: headers_to_map(headers),
            account: account.clone(),
            config: cloaked_config(state),
        })
        .await
    {
        Ok(response) => {
            record_json_result(state, account.provider.clone(), account, &response, model).await;
            json_upstream_response(response, &account.provider, route, model, &BTreeMap::new())
        }
        Err(error) => {
            upstream_failure_response(state, account.provider.clone(), account, &error).await
        }
    }
}

async fn route_anthropic_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    route: RequestRoute,
    model: &str,
    account: &AvailableAccount,
    client_wants_stream: bool,
) -> Response {
    let body = anthropic_request_body(body, model, route);
    // Masquerade openclaw's own tool names and bot-persona system prompt as a
    // first-party Claude Code request so the subscription billing classifier does
    // not reject it. Only the Messages route carries these; the reverse map
    // restores tool_use names in the response.
    let (body, tool_reverse) = if matches!(route, RequestRoute::Messages) {
        let (masked, reverse) = masquerade_request(&body);
        (masked, Arc::new(reverse))
    } else {
        (body, Arc::new(BTreeMap::new()))
    };
    if client_wants_stream {
        return match state
            .upstream
            .anthropic_messages_stream(UpstreamRequest {
                body,
                request_headers: headers_to_map(headers),
                account: account.clone(),
                config: cloaked_config(state),
            })
            .await
        {
            Ok(response) => {
                let accounting = stream_accounting(
                    state,
                    account.provider.clone(),
                    account,
                    response.status,
                    model,
                )
                .await;
                sse_upstream_response(
                    response,
                    account.provider.clone(),
                    route,
                    model,
                    accounting,
                    tool_reverse,
                )
            }
            Err(error) => {
                upstream_failure_response(state, account.provider.clone(), account, &error).await
            }
        };
    }
    match state
        .upstream
        .anthropic_messages(UpstreamRequest {
            body,
            request_headers: headers_to_map(headers),
            account: account.clone(),
            config: cloaked_config(state),
        })
        .await
    {
        Ok(response) => {
            record_json_result(state, account.provider.clone(), account, &response, model).await;
            json_upstream_response(response, &account.provider, route, model, &tool_reverse)
        }
        Err(error) => {
            upstream_failure_response(state, account.provider.clone(), account, &error).await
        }
    }
}

async fn stream_accounting(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    status: StatusCode,
    model: &str,
) -> Option<StreamAccounting> {
    if status.is_success() {
        Some(StreamAccounting {
            state: state.clone(),
            provider,
            account: account.clone(),
            model: model.to_string(),
        })
    } else {
        record_provider_failure(state, provider, account, status, None).await;
        None
    }
}

async fn record_json_result(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    response: &UpstreamJsonResponse,
    model: &str,
) {
    if response.status.is_success() {
        record_provider_success(
            state,
            provider,
            account,
            usage_from_response(&response.body),
            model,
        )
        .await;
    } else {
        record_provider_failure(state, provider, account, response.status, None).await;
    }
}

async fn upstream_failure_response(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    error: &anyhow::Error,
) -> Response {
    record_provider_failure(
        state,
        provider.clone(),
        account,
        StatusCode::BAD_GATEWAY,
        Some(&error.to_string()),
    )
    .await;
    upstream_error_response(provider, error)
}

fn require_api_key(
    state: &AppState,
    headers: &HeaderMap,
    apply_rate_limit: bool,
) -> Result<(), AppError> {
    let Some(api_key) = extract_api_key(headers) else {
        return Err(AppError::simple(
            StatusCode::UNAUTHORIZED,
            "missing API key",
        ));
    };
    if !state.config.api_keys.contains(&api_key) {
        return Err(AppError::simple(StatusCode::FORBIDDEN, "invalid API key"));
    }
    if apply_rate_limit {
        enforce_rate_limit(state, headers)?;
    }
    Ok(())
}

fn enforce_rate_limit(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let key = rate_limit_key(headers);
    let now = Instant::now();
    let mut buckets = state.rate_limit_buckets.lock().map_err(|_| {
        AppError::simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rate-limit bucket lock is poisoned",
        )
    })?;
    let bucket = buckets.entry(key).or_insert(RateLimitBucket {
        count: 0,
        reset_at: now + RATE_LIMIT_WINDOW,
    });
    if now > bucket.reset_at {
        bucket.count = 1;
        bucket.reset_at = now + RATE_LIMIT_WINDOW;
        return Ok(());
    }
    bucket.count += 1;
    if bucket.count > RATE_LIMIT_MAX {
        return Err(AppError::simple(
            StatusCode::TOO_MANY_REQUESTS,
            "too many requests",
        ));
    }
    Ok(())
}

fn rate_limit_key(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("unknown")
        .to_string()
}

fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn enforce_body_limit(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let BodyLimit::Limited(limit) = state.body_limit else {
        return match state.body_limit {
            BodyLimit::Invalid => Err(AppError::simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid body-limit",
            )),
            BodyLimit::Unlimited => Ok(()),
            BodyLimit::Limited(_) => unreachable!(),
        };
    };

    let Some(content_length) = headers.get(CONTENT_LENGTH) else {
        return Err(AppError::simple(
            StatusCode::LENGTH_REQUIRED,
            "missing content-length",
        ));
    };
    let declared_length = content_length
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| AppError::simple(StatusCode::BAD_REQUEST, "invalid content-length"))?;
    if declared_length > limit {
        return Err(AppError::simple(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body too large",
        ));
    }
    Ok(())
}

async fn next_provider_account(
    state: &AppState,
    provider: ProviderId,
) -> Result<AvailableAccount, AppError> {
    let mut manager = match provider.kind {
        ProviderKind::Anthropic => state.account_managers.anthropic.lock().await,
        ProviderKind::Codex => state.account_managers.codex.lock().await,
        ProviderKind::Generic => {
            let Some(manager) = state.account_managers.generic.get(provider.id.as_ref()) else {
                return Err(AppError::provider(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("no available {provider} account; run login for {provider}"),
                    "no_account_for_provider",
                    provider,
                ));
            };
            manager.lock().await
        }
    };
    let result = manager.next_account_result();
    let Some(account) = result.account else {
        return Err(AppError::provider(
            StatusCode::SERVICE_UNAVAILABLE,
            no_account_message(
                &provider,
                result.failure_kind.as_deref(),
                result.retry_after_seconds,
            ),
            "no_account_for_provider",
            provider,
        ));
    };
    let email = account.token.email.clone();
    manager.record_attempt(&email);
    match manager.refresh_if_due(&email).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(AppError::provider(
                StatusCode::BAD_GATEWAY,
                format!("failed to refresh {provider} account; re-run login for {provider}"),
                "token_refresh_failed",
                provider,
            ));
        }
        Err(error) => {
            manager.record_failure(&email, "auth", Some(&error.to_string()));
            return Err(AppError::provider(
                StatusCode::BAD_GATEWAY,
                format!("failed to refresh {provider} account: {error}"),
                "token_refresh_failed",
                provider,
            ));
        }
    }
    Ok(manager.account(&email).unwrap_or(account))
}

async fn record_provider_success(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    usage: Option<UsageData>,
    model: &str,
) {
    let mut manager = match provider.kind {
        ProviderKind::Anthropic => state.account_managers.anthropic.lock().await,
        ProviderKind::Codex => state.account_managers.codex.lock().await,
        ProviderKind::Generic => {
            let Some(manager) = state.account_managers.generic.get(provider.id.as_ref()) else {
                return;
            };
            manager.lock().await
        }
    };
    manager.record_success(account.token.email.as_str(), usage.as_ref(), model);
}

async fn record_provider_failure(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    status: StatusCode,
    detail: Option<&str>,
) {
    // 400/402 by themselves say nothing about account health — a malformed request is
    // the client's fault, and a drained balance is recorded by the failover path with
    // the billing kind instead. Recording both would double-count the backoff.
    if status == StatusCode::BAD_REQUEST || status == StatusCode::PAYMENT_REQUIRED {
        return;
    }
    record_provider_failure_kind(state, provider, account, classify_status(status), detail).await;
}

async fn record_provider_failure_kind(
    state: &AppState,
    provider: ProviderId,
    account: &AvailableAccount,
    kind: &'static str,
    detail: Option<&str>,
) {
    let mut manager = match provider.kind {
        ProviderKind::Anthropic => state.account_managers.anthropic.lock().await,
        ProviderKind::Codex => state.account_managers.codex.lock().await,
        ProviderKind::Generic => {
            let Some(manager) = state.account_managers.generic.get(provider.id.as_ref()) else {
                return;
            };
            manager.lock().await
        }
    };
    manager.record_failure(account.token.email.as_str(), kind, detail);
}

fn no_account_message(
    provider: &ProviderId,
    failure_kind: Option<&str>,
    retry_after_seconds: Option<f64>,
) -> String {
    let mut message = format!("no available {provider} account; run login for {provider}");
    if let Some(failure_kind) = failure_kind {
        write!(message, "; last failure: {failure_kind}").expect("write to String cannot fail");
    }
    if let Some(retry_after_seconds) = retry_after_seconds {
        write!(
            message,
            "; retry after {} seconds",
            retry_after_seconds.ceil()
        )
        .expect("write to String cannot fail");
    }
    message
}

fn classify_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "auth",
        403 => "forbidden",
        429 => "rate_limit",
        500..=599 => "server",
        _ => "network",
    }
}

/// Extract token usage from a provider response, accepting both schema families:
/// Anthropic-native (`input_tokens`, `cache_read_input_tokens`, …) and OpenAI-style
/// (`prompt_tokens`/`completion_tokens` with `prompt_tokens_details`/`completion_tokens_details`).
fn usage_from_response(body: &Value) -> Option<UsageData> {
    let usage = body.get("usage")?;
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(UsageData {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        cache_read_input_tokens: usage
            .get("cache_read_input_tokens")
            .or_else(|| {
                usage
                    .get("input_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
            })
            .or_else(|| {
                usage
                    .get("prompt_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
            })
            .and_then(Value::as_i64)
            .unwrap_or(0),
        reasoning_output_tokens: usage
            .get("output_tokens_details")
            .and_then(|details| {
                // OpenAI-style `reasoning_tokens`; Anthropic (with the
                // thinking-token-count beta) reports `thinking_tokens`.
                details
                    .get("reasoning_tokens")
                    .or_else(|| details.get("thinking_tokens"))
            })
            .or_else(|| {
                usage
                    .get("completion_tokens_details")
                    .and_then(|details| details.get("reasoning_tokens"))
            })
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

fn upstream_error_response(provider: ProviderId, error: &anyhow::Error) -> Response {
    AppError::provider(
        StatusCode::BAD_GATEWAY,
        format!("upstream request failed: {error}"),
        "network_error",
        provider,
    )
    .into_response()
}

fn json_upstream_response(
    response: UpstreamJsonResponse,
    provider: &ProviderId,
    route: RequestRoute,
    model: &str,
    tool_reverse: &BTreeMap<String, String>,
) -> Response {
    if !response.status.is_success() {
        return (response.status, Json(response.body)).into_response();
    }
    let body = match (provider.kind, route) {
        (ProviderKind::Anthropic, RequestRoute::Chat) => anthropic_to_openai(&response.body, model),
        (ProviderKind::Anthropic, RequestRoute::Responses) => {
            anthropic_to_responses(&response.body, model)
        }
        (ProviderKind::Anthropic, RequestRoute::Messages) => {
            let mut body = response.body;
            restore_tool_use_names(&mut body, tool_reverse);
            body
        }
        (ProviderKind::Codex, RequestRoute::Responses) => response.body,
        (ProviderKind::Codex, RequestRoute::Chat) => {
            responses_to_chat_completion(&response.body, model)
        }
        (ProviderKind::Codex, RequestRoute::Messages) => {
            responses_to_anthropic_message(&response.body, model)
        }
        // A generic endpoint speaks Chat Completions; its success body passes
        // through unchanged (slice 4 tests this path end to end). The arm is
        // unreachable until generic routing lands, but it is the honest shape of
        // the response matrix: Generic only ever arrives with Chat, and that
        // body is already a Chat completion.
        #[allow(clippy::match_same_arms)]
        (ProviderKind::Generic, _) => response.body,
    };
    (response.status, Json(body)).into_response()
}

fn sse_upstream_response(
    response: UpstreamSseResponse,
    provider: ProviderId,
    route: RequestRoute,
    model: &str,
    accounting: Option<StreamAccounting>,
    tool_reverse: Arc<BTreeMap<String, String>>,
) -> Response {
    let body = if response.status.is_success() {
        transformed_sse_stream(
            response.body,
            provider.clone(),
            route,
            model.to_string(),
            accounting,
            tool_reverse,
        )
    } else {
        response.body
    };
    Response::builder()
        .status(response.status)
        .header(CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .body(Body::from_stream(body))
        .unwrap_or_else(|error| {
            AppError::provider(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build stream response: {error}"),
                "internal_error",
                provider,
            )
            .into_response()
        })
}

fn transformed_sse_stream(
    mut input: UpstreamSseStream,
    provider: ProviderId,
    route: RequestRoute,
    model: String,
    accounting: Option<StreamAccounting>,
    tool_reverse: Arc<BTreeMap<String, String>>,
) -> UpstreamSseStream {
    Box::pin(try_stream! {
        let mut buffer = Vec::new();
        let mut chat_state = ChatStreamState::new(model.clone());
        let mut responses_state = ResponsesStreamState::new(model.clone());
        let mut anthropic_state = AnthropicStreamState::new(model.clone());
        let mut usage = crate::types::UsageData::default();
        let mut completed = false;
        let mut refusal_next_index: u64 = 0;
        let mut refusal_open_index: Option<u64> = None;

        while let Some(chunk) = input.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    record_stream_failure(accounting.as_ref(), &error.to_string()).await;
                    Err(error)?;
                    unreachable!();
                }
            };
            buffer.extend_from_slice(&chunk);
            let events = match drain_complete_sse_events(&mut buffer) {
                Ok(events) => events,
                Err(error) => {
                    record_stream_failure(accounting.as_ref(), &error.to_string()).await;
                    Err(error)?;
                    unreachable!();
                }
            };
            for (event, raw) in events {
                update_stream_usage(&provider, &event, &raw, &mut usage, &mut completed);
                let chunks = match forward_refusal_event(
                    &provider,
                    route,
                    &event,
                    &raw,
                    &mut refusal_next_index,
                    &mut refusal_open_index,
                ) {
                    Some(replacement) => replacement,
                    None => transform_sse_event(
                        &provider,
                        route,
                        &model,
                        &mut chat_state,
                        &mut responses_state,
                        &mut anthropic_state,
                        &event,
                        &raw,
                        &tool_reverse,
                    ),
                };
                for chunk in chunks {
                    yield Bytes::from(chunk);
                }
            }
        }
        let events = match finish_sse_events(&mut buffer) {
            Ok(events) => events,
            Err(error) => {
                record_stream_failure(accounting.as_ref(), &error.to_string()).await;
                Err(error)?;
                unreachable!();
            }
        };
        for (event, raw) in events {
            update_stream_usage(&provider, &event, &raw, &mut usage, &mut completed);
            let chunks = match forward_refusal_event(
                &provider,
                route,
                &event,
                &raw,
                &mut refusal_next_index,
                &mut refusal_open_index,
            ) {
                Some(replacement) => replacement,
                None => transform_sse_event(
                    &provider,
                    route,
                    &model,
                    &mut chat_state,
                    &mut responses_state,
                    &mut anthropic_state,
                    &event,
                    &raw,
                    &tool_reverse,
                ),
            };
            for chunk in chunks {
                yield Bytes::from(chunk);
            }
        }
        if completed {
            record_stream_success(accounting.as_ref(), &usage).await;
        } else {
            record_stream_failure(accounting.as_ref(), "stream terminated before completion").await;
        }
    })
}

async fn record_stream_success(accounting: Option<&StreamAccounting>, usage: &UsageData) {
    if let Some(accounting) = accounting {
        record_provider_success(
            &accounting.state,
            accounting.provider.clone(),
            &accounting.account,
            Some(usage.clone()),
            &accounting.model,
        )
        .await;
    }
}

async fn record_stream_failure(accounting: Option<&StreamAccounting>, detail: &str) {
    if let Some(accounting) = accounting {
        record_provider_failure(
            &accounting.state,
            accounting.provider.clone(),
            &accounting.account,
            StatusCode::BAD_GATEWAY,
            Some(detail),
        )
        .await;
    }
}

fn update_stream_usage(
    provider: &ProviderId,
    event: &str,
    raw: &str,
    usage: &mut UsageData,
    completed: &mut bool,
) {
    if raw == "[DONE]" {
        *completed = true;
        return;
    }
    let Ok(data) = serde_json::from_str::<Value>(raw) else {
        return;
    };
    match provider.kind {
        ProviderKind::Anthropic => update_anthropic_stream_usage(event, &data, usage, completed),
        ProviderKind::Codex => update_codex_stream_usage(event, &data, usage, completed),
        // A generic endpoint streams Chat Completions chunks; servers that
        // opt into usage carry it on a late chunk (often the last before
        // [DONE]) in the same shape as the non-streamed body.
        ProviderKind::Generic => update_generic_stream_usage(&data, usage),
    }
}

fn update_generic_stream_usage(data: &Value, usage: &mut UsageData) {
    let Some(next) = data.get("usage") else {
        return;
    };
    // Chunks without usage (or with null fields) must not clobber what
    // earlier chunks recorded.
    usage.input_tokens = int_field_or(next, "prompt_tokens", usage.input_tokens);
    usage.output_tokens = int_field_or(next, "completion_tokens", usage.output_tokens);
    usage.cache_read_input_tokens = next
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(usage.cache_read_input_tokens);
    usage.reasoning_output_tokens = next
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(usage.reasoning_output_tokens);
}

fn update_anthropic_stream_usage(
    event: &str,
    data: &Value,
    usage: &mut UsageData,
    completed: &mut bool,
) {
    match event {
        "message_start" => {
            if let Some(payload) = data.get("message").and_then(|message| message.get("usage")) {
                usage.input_tokens = int_field_or(payload, "input_tokens", usage.input_tokens);
                usage.cache_creation_input_tokens = int_field_or(
                    payload,
                    "cache_creation_input_tokens",
                    usage.cache_creation_input_tokens,
                );
                usage.cache_read_input_tokens = int_field_or(
                    payload,
                    "cache_read_input_tokens",
                    usage.cache_read_input_tokens,
                );
            }
        }
        "message_delta" => {
            if let Some(payload) = data.get("usage") {
                usage.output_tokens = int_field_or(payload, "output_tokens", usage.output_tokens);
                usage.reasoning_output_tokens = payload
                    .get("output_tokens_details")
                    .and_then(|details| details.get("thinking_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(usage.reasoning_output_tokens);
            }
        }
        "message_stop" => *completed = true,
        _ => {}
    }
}

fn update_codex_stream_usage(
    event: &str,
    data: &Value,
    usage: &mut UsageData,
    completed: &mut bool,
) {
    if matches!(event, "response.completed" | "response.incomplete") {
        *completed = true;
        let response = data.get("response").unwrap_or(data);
        if let Some(next_usage) = usage_from_response(response) {
            *usage = next_usage;
        }
    }
}

fn int_field_or(value: &Value, key: &str, default: i64) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(default)
}

/// Turn an Anthropic streaming refusal into a deliverable assistant text message.
///
/// A refusal arrives as a `message_delta` with `delta.stop_reason == "refusal"` and
/// no content, which openclaw surfaces to the user only as a generic "LLM request
/// failed". To make the reason visible in the chat, inject a text block carrying it
/// and rewrite the stop reason to `end_turn` so the turn reads as a normal
/// completion. `next_index`/`open_index` track the content-block cursor so the
/// injected block lands on a free index (and any open block is closed first).
/// Returns the replacement SSE chunks for a refusal event, or `None` to fall
/// through to the normal transform.
fn forward_refusal_event(
    provider: &ProviderId,
    route: RequestRoute,
    event: &str,
    raw: &str,
    next_index: &mut u64,
    open_index: &mut Option<u64>,
) -> Option<Vec<String>> {
    if provider.kind != ProviderKind::Anthropic || !matches!(route, RequestRoute::Messages) {
        return None;
    }
    let data = serde_json::from_str::<Value>(raw).ok()?;
    match event {
        "content_block_start" => {
            if let Some(i) = data.get("index").and_then(Value::as_u64) {
                *next_index = i + 1;
                *open_index = Some(i);
            }
            None
        }
        "content_block_stop" => {
            *open_index = None;
            None
        }
        "message_delta"
            if data.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("refusal") =>
        {
            let category = data
                .pointer("/delta/stop_details/category")
                .and_then(Value::as_str)
                .unwrap_or("unspecified");
            let reason = format!(
                "⚠️ Upstream refusal: Anthropic declined to generate a response (safety category: {category})."
            );
            let mut out = Vec::new();
            if let Some(open) = open_index.take() {
                out.push(sse(
                    &serde_json::json!({"type": "content_block_stop", "index": open}),
                    Some("content_block_stop"),
                ));
            }
            let idx = *next_index;
            out.push(sse(
                &serde_json::json!({
                    "type": "content_block_start", "index": idx,
                    "content_block": {"type": "text", "text": ""}
                }),
                Some("content_block_start"),
            ));
            out.push(sse(
                &serde_json::json!({
                    "type": "content_block_delta", "index": idx,
                    "delta": {"type": "text_delta", "text": reason}
                }),
                Some("content_block_delta"),
            ));
            out.push(sse(
                &serde_json::json!({"type": "content_block_stop", "index": idx}),
                Some("content_block_stop"),
            ));
            let mut fixed = data;
            fixed["delta"]["stop_reason"] = Value::String("end_turn".to_string());
            fixed["delta"]["stop_details"] = Value::Null;
            out.push(sse(&fixed, Some("message_delta")));
            Some(out)
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn transform_sse_event(
    provider: &ProviderId,
    route: RequestRoute,
    model: &str,
    chat_state: &mut ChatStreamState,
    responses_state: &mut ResponsesStreamState,
    anthropic_state: &mut AnthropicStreamState,
    event: &str,
    raw: &str,
    tool_reverse: &BTreeMap<String, String>,
) -> Vec<String> {
    if raw == "[DONE]" {
        return match (provider.kind, route) {
            (ProviderKind::Anthropic, RequestRoute::Messages)
            | (ProviderKind::Codex, RequestRoute::Responses) => {
                vec!["data: [DONE]\n\n".to_string()]
            }
            // Chat Completions clients — generic upstreams included — end
            // their stream with [DONE]; dropping it hangs parsers that wait
            // for the terminator.
            _ => vec!["data: [DONE]\n\n".to_string()],
        };
    }

    let parsed = serde_json::from_str::<Value>(raw);
    match (provider.kind, route) {
        (ProviderKind::Anthropic, RequestRoute::Messages)
        | (ProviderKind::Codex, RequestRoute::Responses) => parsed.map_or_else(
            |_| {
                vec![sse(
                    &Value::String(raw.to_string()),
                    passthrough_event(event),
                )]
            },
            |mut data| {
                restore_tool_use_names(&mut data, tool_reverse);
                vec![sse(&data, passthrough_event(event))]
            },
        ),
        (ProviderKind::Anthropic, RequestRoute::Chat) => parsed.map_or_else(
            |_| Vec::new(),
            |data| anthropic_sse_to_chat(event, &data, chat_state),
        ),
        (ProviderKind::Anthropic, RequestRoute::Responses) => parsed.map_or_else(
            |_| Vec::new(),
            |data| anthropic_sse_to_responses(event, &data, responses_state, model),
        ),
        (ProviderKind::Codex, RequestRoute::Chat) => parsed.map_or_else(
            |_| Vec::new(),
            |data| responses_sse_to_chat(event, &data, chat_state),
        ),
        (ProviderKind::Codex, RequestRoute::Messages) => parsed.map_or_else(
            |_| Vec::new(),
            |data| responses_sse_to_anthropic(event, &data, anthropic_state),
        ),
        // A generic endpoint's Chat Completions stream passes through unchanged
        // (slice 4 tests this path end to end).
        (ProviderKind::Generic, RequestRoute::Chat) => parsed.map_or_else(
            |_| Vec::new(),
            |data| vec![sse(&data, passthrough_event(event))],
        ),
        (ProviderKind::Generic, _) => parsed.map_or_else(
            |_| Vec::new(),
            |data| vec![sse(&data, passthrough_event(event))],
        ),
    }
}

fn passthrough_event(event: &str) -> Option<&str> {
    (event != "message").then_some(event)
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn body_with_model(body: &Value, model: &str) -> Value {
    let mut next_body = body.clone();
    if let Some(object) = next_body.as_object_mut() {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }
    next_body
}

fn anthropic_request_body(body: &Value, model: &str, route: RequestRoute) -> Value {
    let translated = match route {
        RequestRoute::Chat => openai_to_anthropic(body),
        RequestRoute::Responses => responses_to_anthropic(body),
        RequestRoute::Messages => body.clone(),
    };
    body_with_model(&translated, model)
}

fn codex_request_body(body: &Value, model: &str, route: RequestRoute) -> Value {
    let translated = match route {
        RequestRoute::Chat => chat_to_responses_request(body),
        RequestRoute::Responses => body.clone(),
        RequestRoute::Messages => anthropic_to_responses_request(body),
    };
    let mut normalized = normalize_codex_responses_body(&body_with_model(&translated, model));
    if let Some(object) = normalized.as_object_mut() {
        object.insert("stream".to_string(), Value::Bool(true));
        object.remove("max_output_tokens");
        object.remove("parallel_tool_calls");
    }
    normalized
}

/// Build a POST request with a JSON body and provider headers.
///
/// `.json()` already sets `Content-Type: application/json`, so any `content-type` entry in
/// `headers` is skipped to avoid sending a duplicate header. The Codex backend rejects a
/// duplicate `Content-Type` with "Unsupported content type".
fn build_upstream_request(
    client: &reqwest::Client,
    url: &str,
    headers: BTreeMap<String, String>,
    body: &Value,
    timeout_ms: u64,
) -> reqwest::RequestBuilder {
    tracing::debug!(%url, "upstream request");
    let mut request = client
        .post(url)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .json(body);
    for (key, value) in headers {
        if key.eq_ignore_ascii_case("content-type") {
            continue;
        }
        request = request.header(key, value);
    }
    request
}

async fn send_json(
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    body: Value,
    timeout_ms: u64,
) -> anyhow::Result<UpstreamJsonResponse> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("claude-sonnet-4-6")
        .to_string();
    let response = build_upstream_request(&client, &url, headers, &body, timeout_ms)
        .send()
        .await?;
    let mut status = StatusCode::from_u16(response.status().as_u16())?;
    let headers = response.headers().clone();
    let bytes = response.bytes().await?;
    let body = decode_upstream_body(&headers, &bytes, &model);
    if status.is_success() && is_decoded_upstream_error(&body) {
        status = StatusCode::BAD_GATEWAY;
    }
    if status.is_success() {
        tracing::debug!(%url, model = %model, status = status.as_u16(), "upstream response");
    } else {
        tracing::warn!(%url, model = %model, status = status.as_u16(), "upstream error response");
    }
    Ok(UpstreamJsonResponse { status, body })
}

async fn send_get(
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    timeout_ms: u64,
) -> anyhow::Result<Value> {
    let mut request = client
        .get(&url)
        .timeout(std::time::Duration::from_millis(timeout_ms));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        anyhow::bail!("GET {url} returned {status}");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn send_stream(
    client: reqwest::Client,
    url: String,
    headers: BTreeMap<String, String>,
    body: Value,
    timeout_ms: u64,
) -> anyhow::Result<UpstreamSseResponse> {
    let response = build_upstream_request(&client, &url, headers, &body, timeout_ms)
        .send()
        .await?;
    let status = StatusCode::from_u16(response.status().as_u16())?;
    if status.is_success() {
        tracing::debug!(%url, status = status.as_u16(), "upstream stream opened");
    } else {
        tracing::warn!(%url, status = status.as_u16(), "upstream stream error");
    }
    Ok(UpstreamSseResponse {
        status,
        body: Box::pin(response.bytes_stream().map_err(anyhow::Error::from)),
    })
}

fn decode_upstream_body(headers: &HeaderMap, bytes: &[u8], model: &str) -> Value {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if is_event_stream_content_type(content_type) || looks_like_event_stream_body(bytes) {
        return responses_sse_to_payload(&[bytes], model).unwrap_or_else(|error| {
            json!({
                "error": {
                    "message": format!("failed to parse upstream event stream: {error}")
                }
            })
        });
    }

    serde_json::from_slice(bytes).unwrap_or_else(|_| {
        json!({
            "error": {
                "message": String::from_utf8_lossy(bytes)
            }
        })
    })
}

fn is_event_stream_content_type(content_type: &str) -> bool {
    content_type
        .to_ascii_lowercase()
        .starts_with("text/event-stream")
}

fn looks_like_event_stream_body(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|body| {
        let body = body.trim_start();
        body.starts_with("event:") || body.starts_with("data:")
    })
}

fn is_decoded_upstream_error(body: &Value) -> bool {
    body.get("error")
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        == Some("upstream_error")
}

fn non_empty_array(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|value| !value.is_empty())
}

/// The request's `model`, if present and not blank. A request without one is rejected
/// rather than defaulted to a model the caller did not ask for.
fn required_model(body: &Value) -> Option<&str> {
    body.get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

fn parse_body_limit(value: &str) -> BodyLimit {
    let raw = value.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return BodyLimit::Unlimited;
    }
    for (suffix, multiplier) in [
        ("gb", 1024_u64 * 1024 * 1024),
        ("mb", 1024_u64 * 1024),
        ("kb", 1024_u64),
        ("b", 1_u64),
    ] {
        if let Some(number) = raw.strip_suffix(suffix) {
            return parse_limit_number(number.trim(), multiplier);
        }
    }
    parse_limit_number(&raw, 1)
}

fn parse_limit_number(number: &str, multiplier: u64) -> BodyLimit {
    let Ok(value) = number.parse::<u64>() else {
        return BodyLimit::Invalid;
    };
    value
        .checked_mul(multiplier)
        .map_or(BodyLimit::Invalid, BodyLimit::Limited)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use anyhow::{Result, bail};
    use axum::http::{HeaderMap, StatusCode};
    use http_body_util::BodyExt;

    use super::{
        AccountManagers, AppState, BodyLimit, ModelsFuture, RateLimitBucket, RequestRoute,
        StdRwLock, UpstreamClient, UpstreamFuture, UpstreamJsonResponse, UpstreamRequest,
        UpstreamSseFuture, build_upstream_request, decode_upstream_body, forward_refusal_event,
        is_decoded_upstream_error, refresh_model_catalog, route_provider_request,
    };
    use crate::accounts::{AccountManager, RefreshPolicy};
    use crate::config::{CloakingConfig, Config, DebugMode, TimeoutConfig};
    use crate::models::{FetchedModels, ModelCatalog};
    use crate::tokens::save_token;
    use crate::types::{AvailableAccount, ProviderId, ProviderKind, TokenData};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    #[test]
    fn refusal_stream_event_becomes_deliverable_text() {
        let mut next = 0u64;
        let mut open = None;
        let raw = r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"category":"bio","type":"refusal"}}}"#;
        let out = forward_refusal_event(
            &ProviderId::anthropic(),
            RequestRoute::Messages,
            "message_delta",
            raw,
            &mut next,
            &mut open,
        )
        .expect("refusal is replaced");
        let joined = out.join("");
        assert!(
            joined.contains("content_block_start"),
            "injects a text block"
        );
        assert!(
            joined.contains("safety category: bio"),
            "carries the reason"
        );
        assert!(
            joined.contains("\"stop_reason\":\"end_turn\""),
            "stop_reason rewritten so openclaw treats it as a normal completion"
        );
    }

    #[test]
    fn non_refusal_events_pass_through() {
        let mut next = 0u64;
        let mut open = None;
        // a normal stop passes through (None → normal transform)
        let normal = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#;
        assert!(
            forward_refusal_event(
                &ProviderId::anthropic(),
                RequestRoute::Messages,
                "message_delta",
                normal,
                &mut next,
                &mut open,
            )
            .is_none()
        );
        // content_block_start advances the injection cursor past the used index
        forward_refusal_event(
            &ProviderId::anthropic(),
            RequestRoute::Messages,
            "content_block_start",
            r#"{"type":"content_block_start","index":0}"#,
            &mut next,
            &mut open,
        );
        assert_eq!(next, 1, "next injection index moves past block 0");
        assert_eq!(open, Some(0));
    }

    #[test]
    fn upstream_request_sends_single_content_type() {
        let headers = BTreeMap::from([
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "text/event-stream".to_string()),
            ("Authorization".to_string(), "Bearer token".to_string()),
        ]);
        let body = json!({"model": "gpt-5.5", "stream": true});
        let request = build_upstream_request(
            &reqwest::Client::new(),
            "https://chatgpt.com/backend-api/codex/responses",
            headers,
            &body,
            30_000,
        )
        .build()
        .expect("request builds");
        let content_types: Vec<_> = request
            .headers()
            .get_all(reqwest::header::CONTENT_TYPE)
            .iter()
            .collect();
        assert_eq!(content_types.len(), 1, "exactly one Content-Type header");
        assert_eq!(content_types[0], "application/json");
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::ACCEPT)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream"),
        );
    }

    #[derive(Default)]
    struct CapturingUpstream {
        calls: Mutex<Vec<String>>,
        fetch_fails: std::sync::atomic::AtomicBool,
    }

    impl UpstreamClient for CapturingUpstream {
        fn generic_chat(&self, _request: UpstreamRequest) -> UpstreamFuture {
            unreachable!("generic chat not used in refresh fallback test")
        }

        fn generic_chat_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
            unreachable!("generic stream not used in refresh fallback test")
        }

        fn anthropic_messages(&self, request: UpstreamRequest) -> UpstreamFuture {
            self.calls
                .lock()
                .expect("calls lock")
                .push(request.account.token.email);
            Box::pin(async {
                Ok(UpstreamJsonResponse {
                    status: StatusCode::OK,
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

        fn anthropic_messages_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
            unreachable!("stream not used in refresh fallback test")
        }

        fn anthropic_count_tokens(&self, _request: UpstreamRequest) -> UpstreamFuture {
            unreachable!("count_tokens not used in refresh fallback test")
        }

        fn codex_responses(&self, _request: UpstreamRequest) -> UpstreamFuture {
            unreachable!("codex not used in refresh fallback test")
        }

        fn codex_responses_stream(&self, _request: UpstreamRequest) -> UpstreamSseFuture {
            unreachable!("codex stream not used in refresh fallback test")
        }

        fn fetch_models(
            &self,
            _kind: ProviderKind,
            _account: AvailableAccount,
            _config: Arc<Config>,
        ) -> ModelsFuture {
            let fails = self.fetch_fails.load(std::sync::atomic::Ordering::Relaxed);
            Box::pin(async move {
                if fails {
                    anyhow::bail!("simulated model list fetch failure");
                }
                Ok(FetchedModels::new(Vec::new()))
            })
        }
    }

    fn token(email: &str, access_token: &str, expires_at: &str) -> TokenData {
        TokenData {
            access_token: access_token.to_string(),
            refresh_token: format!("{access_token}-refresh"),
            email: email.to_string(),
            expires_at: expires_at.to_string(),
            account_uuid: email.to_string(),
            provider: ProviderId::anthropic(),
            id_token: None,
            last_refresh_at: None,
            plan_type: None,
        }
    }

    #[test]
    fn decode_upstream_body_drains_event_stream_payloads() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );

        let body = decode_upstream_body(
            &headers,
            concat!(
                "event: response.output_text.delta\n",
                "data: {\"delta\":\"ok\"}\n\n",
                "event: response.completed\n",
                "data: {\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.4\"}}\n\n"
            )
            .as_bytes(),
            "gpt-5.4",
        );

        assert_eq!(body["id"], "resp_1");
        assert_eq!(body["output_text"], "ok");
    }

    #[test]
    fn decode_upstream_body_drains_event_stream_payloads_without_event_stream_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );

        let body = decode_upstream_body(
            &headers,
            concat!(
                "event: response.output_text.delta\n",
                "data: {\"delta\":\"ok\"}\n\n",
                "event: response.completed\n",
                "data: {\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.4\"}}\n\n"
            )
            .as_bytes(),
            "gpt-5.4",
        );

        assert_eq!(body["id"], "resp_1");
        assert_eq!(body["output_text"], "ok");
    }

    #[test]
    fn decode_upstream_body_flags_event_stream_failures() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );

        let body = decode_upstream_body(
            &headers,
            concat!(
                "event: response.failed\n",
                "data: {\"error\":{\"message\":\"model overloaded\"}}\n\n"
            )
            .as_bytes(),
            "gpt-5.4",
        );

        assert!(is_decoded_upstream_error(&body));
        assert_eq!(body["error"]["message"], "model overloaded");
    }

    #[tokio::test]
    async fn route_tries_next_account_when_first_refresh_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        save_token(
            tmp.path(),
            &token(
                "alice@example.com",
                "anthropic-access-alice",
                "2000-01-01T00:00:00Z",
            ),
        )
        .expect("save alice");
        save_token(
            tmp.path(),
            &token(
                "bob@example.com",
                "anthropic-access-bob",
                "2030-01-01T00:00:00Z",
            ),
        )
        .expect("save bob");

        let upstream = Arc::new(CapturingUpstream::default());
        let state = test_state(tmp.path(), upstream.clone());

        let response = route_provider_request(
            &state,
            &HeaderMap::new(),
            &json!({
                "model": "claude-sonnet-4-6",
                "messages": [{"role": "user", "content": "reply exactly: pong"}]
            }),
            RequestRoute::Messages,
        )
        .await;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = serde_json::from_slice::<Value>(&body).expect("json body");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["content"][0]["text"], "pong");
        assert_eq!(
            *upstream.calls.lock().expect("calls lock"),
            ["bob@example.com"]
        );
    }

    fn test_config(auth_dir: std::path::PathBuf) -> Config {
        Config {
            host: String::new(),
            port: 8317,
            auth_dir,
            api_keys: std::collections::HashSet::new(),
            body_limit: String::new(),
            cloaking: CloakingConfig {
                cli_version: "2.1.88".to_string(),
                entrypoint: "cli".to_string(),
                codex: std::collections::BTreeMap::new(),
            },
            timeouts: TimeoutConfig {
                messages_ms: 120_000,
                stream_messages_ms: 600_000,
                count_tokens_ms: 30_000,
            },
            stats_enabled: true,
            debug: DebugMode::Off,
            providers: std::collections::BTreeMap::new(),
        }
    }

    fn manager(auth_dir: &std::path::Path, provider: ProviderId) -> AccountManager {
        let mut manager = AccountManager::new(
            auth_dir.to_path_buf(),
            provider,
            |_refresh_token| {
                Box::pin(async {
                    bail!("unused refresh");
                }) as Pin<Box<dyn Future<Output = Result<TokenData>> + Send>>
            },
            RefreshPolicy::default(),
        );
        let _ = manager.load();
        manager
    }

    fn test_state(tmp: &std::path::Path, upstream: Arc<CapturingUpstream>) -> AppState {
        let config = Arc::new(test_config(tmp.to_path_buf()));
        AppState {
            cloaking: Arc::new(StdRwLock::new(super::Cloaking::new(
                &config,
                crate::cloaking_versions::CliVersions::default(),
            ))),
            config,
            body_limit: BodyLimit::Unlimited,
            upstream,
            account_managers: Arc::new(AccountManagers {
                anthropic: tokio::sync::Mutex::new(manager(tmp, ProviderId::anthropic())),
                codex: tokio::sync::Mutex::new(manager(tmp, ProviderId::codex())),
                generic: std::collections::BTreeMap::new(),
            }),
            rate_limit_buckets: Arc::new(Mutex::new(std::collections::BTreeMap::<
                String,
                RateLimitBucket,
            >::new())),
            catalog: Arc::new(StdRwLock::new(ModelCatalog::default())),
        }
    }

    #[tokio::test]
    async fn removed_provider_model_ids_are_unknown_models() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let upstream = Arc::new(CapturingUpstream::default());
        let state = test_state(tmp.path(), upstream.clone());

        let response = route_provider_request(
            &state,
            &HeaderMap::new(),
            &json!({
                "model": "opencode/glm-5.1",
                "messages": [{"role": "user", "content": "hi"}]
            }),
            RequestRoute::Chat,
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(upstream.calls.lock().expect("calls lock").is_empty());

        let response = route_provider_request(
            &state,
            &HeaderMap::new(),
            &json!({
                "model": "opencode/glm-5.1",
                "messages": [{"role": "user", "content": "hi"}]
            }),
            RequestRoute::Messages,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(upstream.calls.lock().expect("calls lock").is_empty());
    }

    #[tokio::test]
    async fn refresh_keeps_the_last_good_catalog_when_a_fetch_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        save_token(
            tmp.path(),
            &token(
                "alice@example.com",
                "anthropic-access",
                "2030-01-01T00:00:00Z",
            ),
        )
        .expect("save anthropic token");
        save_token(
            tmp.path(),
            &TokenData {
                access_token: "codex-access".to_string(),
                refresh_token: "codex-refresh".to_string(),
                email: "bob@example.com".to_string(),
                expires_at: "2030-01-01T00:00:00Z".to_string(),
                account_uuid: "acct-codex".to_string(),
                provider: ProviderId::codex(),
                id_token: None,
                last_refresh_at: None,
                plan_type: None,
            },
        )
        .expect("save codex token");
        let upstream = Arc::new(CapturingUpstream::default());
        let state = test_state(tmp.path(), upstream.clone());

        // Seed a known-good catalog, then force every upstream fetch to fail.
        {
            let mut catalog = state.catalog.write().expect("catalog lock");
            catalog.set_direct(
                ProviderKind::Anthropic,
                FetchedModels::new(vec!["claude-opus-5".into()]),
            );
            catalog.set_direct(
                ProviderKind::Codex,
                FetchedModels::new(vec!["gpt-5.5".into()]),
            );
        }
        upstream
            .fetch_fails
            .store(true, std::sync::atomic::Ordering::Relaxed);

        refresh_model_catalog(&state).await;

        // A failed refresh keeps the last-good set instead of wiping it.
        let advertised: Vec<String> = state
            .catalog
            .read()
            .expect("catalog lock")
            .advertised()
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert!(advertised.contains(&"anthropic/claude-opus-5".to_string()));
        assert!(advertised.contains(&"codex/gpt-5.5".to_string()));
    }

    #[test]
    fn usage_from_response_reads_openai_chat_details() {
        let usage = super::usage_from_response(&json!({
            "usage": {
                "prompt_tokens": 89,
                "completion_tokens": 26,
                "prompt_tokens_details": {"cached_tokens": 7},
                "completion_tokens_details": {"reasoning_tokens": 23}
            }
        }))
        .expect("usage");
        assert_eq!(usage.input_tokens, 89);
        assert_eq!(usage.output_tokens, 26);
        assert_eq!(usage.cache_read_input_tokens, 7);
        assert_eq!(usage.reasoning_output_tokens, 23);
    }

    #[test]
    fn usage_from_response_reads_anthropic_thinking_tokens() {
        // With the thinking-token-count beta, Anthropic reports thinking
        // inside output_tokens_details; it feeds the reasoning counter.
        let usage = super::usage_from_response(&json!({
            "usage": {
                "input_tokens": 64,
                "output_tokens": 229,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
                "output_tokens_details": {"thinking_tokens": 145}
            }
        }))
        .expect("usage");
        assert_eq!(usage.output_tokens, 229);
        assert_eq!(usage.reasoning_output_tokens, 145);
    }

    #[test]
    fn anthropic_stream_usage_reads_thinking_tokens_from_message_delta() {
        let mut usage = crate::types::UsageData::default();
        let mut completed = false;
        super::update_anthropic_stream_usage(
            "message_delta",
            &json!({"usage": {"output_tokens": 72, "output_tokens_details": {"thinking_tokens": 58}}}),
            &mut usage,
            &mut completed,
        );
        assert_eq!(usage.output_tokens, 72);
        assert_eq!(usage.reasoning_output_tokens, 58);
    }

    #[test]
    fn does_not_retry_locally_generated_not_implemented() {
        // 501 is pengepul's own deterministic "unsupported route" response, never a
        // transient upstream signal, so retrying it just re-generates the same error.
        assert!(!super::should_retry_upstream_status(
            StatusCode::NOT_IMPLEMENTED
        ));
        assert!(super::should_retry_upstream_status(
            StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(super::should_retry_upstream_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(super::should_retry_upstream_status(StatusCode::BAD_GATEWAY));
        assert!(!super::should_retry_upstream_status(StatusCode::OK));
    }
}
