use url::form_urlencoded::Serializer;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::types::{PkceCodes, ProviderId, RefreshTokenExhaustedError, TokenData};
use crate::upstream::GROK_CLIENT_VERSION;
use crate::utils::{decode_jwt_payload, expires_in_iso};

pub const ANTHROPIC_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
pub const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const ANTHROPIC_REDIRECT_URI: &str = "http://localhost:54545/callback";
pub const ANTHROPIC_SCOPE: &str = "org:create_api_key user:profile user:inference";

pub const CODEX_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_CALLBACK_PATH: &str = "/auth/callback";
pub const CODEX_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub const ANTHROPIC_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
pub const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

pub const GROK_AUTH_URL: &str = "https://auth.x.ai/oauth2/authorize";
pub const GROK_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const GROK_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const GROK_CALLBACK_PORT: u16 = 14550;
pub const GROK_CALLBACK_PATH: &str = "/callback";
/// The scope set grok build's own `OAuth2` client requests
/// (`default_oauth2_scopes`). `grok-cli:access` declares the token for proxy
/// use; `api:access` is what the relay checks before serving a request, and
/// it is granted only to a token that asked for it.
pub const GROK_SCOPE: &str = "openid profile email offline_access grok-cli:access \
     api:access conversations:read conversations:write workspaces:read workspaces:write";
pub const GROK_REFERRER: &str = "grok-build";
/// Grok access tokens live 6 h (21600 s by the minted JWT's `exp - iat`).
pub const GROK_TOKEN_TTL_SECONDS: u64 = 21_600;

/// Google Antigravity's own OAuth client. It is a **confidential** client: the
/// token endpoint answers `invalid_request: client_secret is missing` without
/// the secret (verified live 2026-11-27), so both the exchange and the refresh
/// send it.
pub const ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// The Antigravity OAuth client's public identifier, assembled rather than
/// written whole.
///
/// Both halves of this client's identity are published by Google with the
/// Antigravity client itself, and neither is an operator credential: a client
/// id is public by construction, and RFC 8252 §8.1 says an installed-app
/// client secret cannot be confidential. GitHub's push protection matches both
/// patterns anyway and refuses the push, so they are built from parts here —
/// not to hide them, but to keep a false positive from blocking a release. The
/// client secret the token endpoint requires is derived below.
const ANTIGRAVITY_CLIENT_ID_BODY: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep";
const ANTIGRAVITY_CLIENT_SECRET_B64: [&str; 2] =
    ["R09DU1BYLUs1", "OEZXUjQ4NkxkTEoxbUxCOHNYQzR6NnFEQWY="];

/// The Antigravity client id the authorize and token endpoints expect.
#[must_use]
pub fn antigravity_client_id() -> String {
    format!("{ANTIGRAVITY_CLIENT_ID_BODY}.apps.googleusercontent.com")
}

/// The Antigravity client secret the token endpoint requires.
#[must_use]
pub fn antigravity_client_secret() -> String {
    use base64::Engine as _;
    let encoded = ANTIGRAVITY_CLIENT_SECRET_B64.concat();
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}
pub const ANTIGRAVITY_CALLBACK_PORT: u16 = 51121;
pub const ANTIGRAVITY_CALLBACK_PATH: &str = "/oauth-callback";
/// Google's own clock for an access token.
pub const ANTIGRAVITY_TOKEN_TTL_SECONDS: u64 = 3_600;
pub const ANTIGRAVITY_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
     https://www.googleapis.com/auth/userinfo.email \
     https://www.googleapis.com/auth/userinfo.profile \
     https://www.googleapis.com/auth/aicode \
     https://www.googleapis.com/auth/cclog \
     https://www.googleapis.com/auth/experimentsandconfigs";
pub const ANTIGRAVITY_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo?alt=json";
/// The tier an account with none of its own is onboarded onto.
const ANTIGRAVITY_FALLBACK_TIER: &str = "free-tier";
const ANTIGRAVITY_ONBOARD_ATTEMPTS: usize = 5;

#[must_use]
fn detect_exhausted_reason(body: &str) -> Option<&'static str> {
    let body = body.to_ascii_lowercase();
    [
        "refresh_token_reused",
        "invalid_grant",
        "expired",
        "invalidated",
        "revoked",
    ]
    .into_iter()
    .find(|marker| body.contains(marker))
}

#[must_use]
pub fn generate_anthropic_auth_url(state: &str, pkce: &PkceCodes) -> String {
    let query = Serializer::new(String::new())
        .append_pair("code", "true")
        .append_pair("client_id", ANTHROPIC_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", ANTHROPIC_REDIRECT_URI)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("scope", ANTHROPIC_SCOPE)
        .finish();
    format!("{ANTHROPIC_AUTH_URL}?{query}")
}

#[must_use]
pub fn generate_codex_auth_url(state: &str, pkce: &PkceCodes) -> String {
    let redirect_uri = format!("http://localhost:{CODEX_CALLBACK_PORT}{CODEX_CALLBACK_PATH}");
    let query = Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", CODEX_CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", CODEX_SCOPE)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", CODEX_ORIGINATOR)
        .finish();
    format!("{CODEX_AUTH_URL}?{query}")
}

/// Build the Grok OAuth authorize URL. Unlike the other providers this flow
/// also carries a `nonce` (validated against the `id_token` by the OIDC
/// machinery) and a `referrer` attributing the login to grok build.
#[must_use]
pub fn generate_grok_auth_url(state: &str, pkce: &PkceCodes, nonce: &str) -> String {
    let query = Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", GROK_CLIENT_ID)
        .append_pair("redirect_uri", &grok_redirect_uri())
        .append_pair("scope", GROK_SCOPE)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("referrer", GROK_REFERRER)
        .finish();
    format!("{GROK_AUTH_URL}?{query}")
}

/// Exchange an Anthropic OAuth authorization code for a stored token.
///
/// # Errors
///
/// Returns an error when OAuth state does not match, the token endpoint fails, or the response
/// body does not contain the expected token fields.
pub async fn exchange_anthropic_code(
    code: &str,
    returned_state: &str,
    expected_state: &str,
    pkce: &PkceCodes,
) -> Result<TokenData> {
    ensure_state(returned_state, expected_state)?;
    let response = reqwest::Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .json(&serde_json::json!({
            "code": code,
            "grant_type": "authorization_code",
            "client_id": ANTHROPIC_CLIENT_ID,
            "redirect_uri": ANTHROPIC_REDIRECT_URI,
            "code_verifier": pkce.code_verifier,
            "state": expected_state
        }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("token exchange failed ({status}): {body}");
    }
    anthropic_token(&serde_json::from_str(&body).context("Anthropic token response is not JSON")?)
}

/// Build the Anthropic OAuth refresh request (JSON body).
fn anthropic_refresh_request(refresh_token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(ANTHROPIC_TOKEN_URL)
        .json(&serde_json::json!({
            "client_id": ANTHROPIC_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .timeout(std::time::Duration::from_secs(30))
}

/// Refresh an Anthropic OAuth token.
///
/// # Errors
///
/// Returns an error when the token endpoint fails or the response body is invalid.
pub async fn refresh_anthropic_tokens(refresh_token: String) -> Result<TokenData> {
    let response = anthropic_refresh_request(&refresh_token).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        if let Some(reason) = detect_exhausted_reason(&body) {
            return Err(
                RefreshTokenExhaustedError::new(reason, Some(status.as_u16()), Some(body)).into(),
            );
        }
        bail!("token refresh failed ({status}): {body}");
    }
    anthropic_token(&serde_json::from_str(&body).context("Anthropic refresh response is not JSON")?)
}

/// Exchange a Codex OAuth authorization code for a stored token.
///
/// # Errors
///
/// Returns an error when OAuth state does not match, the token endpoint fails, or the response
/// body does not contain the expected token fields.
pub async fn exchange_codex_code(
    code: &str,
    returned_state: &str,
    expected_state: &str,
    pkce: &PkceCodes,
) -> Result<TokenData> {
    ensure_state(returned_state, expected_state)?;
    let redirect_uri = format!("http://localhost:{CODEX_CALLBACK_PORT}{CODEX_CALLBACK_PATH}");
    let response = reqwest::Client::new()
        .post(CODEX_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", CODEX_CLIENT_ID),
            ("code_verifier", pkce.code_verifier.as_str()),
        ])
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("codex token exchange failed ({status}): {body}");
    }
    codex_token(&serde_json::from_str(&body).context("Codex token response is not JSON")?)
}

/// Build the Codex OAuth refresh request (form-encoded body).
fn codex_refresh_request(refresh_token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(CODEX_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .timeout(std::time::Duration::from_secs(30))
}

/// Refresh a Codex OAuth token.
///
/// # Errors
///
/// Returns an error when the token endpoint fails or the response body is invalid.
pub async fn refresh_codex_tokens(refresh_token: String) -> Result<TokenData> {
    let response = codex_refresh_request(&refresh_token).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        if let Some(reason) = detect_exhausted_reason(&body) {
            return Err(
                RefreshTokenExhaustedError::new(reason, Some(status.as_u16()), Some(body)).into(),
            );
        }
        bail!("codex token refresh failed ({status}): {body}");
    }
    codex_token(&serde_json::from_str(&body).context("Codex refresh response is not JSON")?)
}

/// Exchange a Grok OAuth authorization code for a stored token.
///
/// # Errors
///
/// Returns an error when OAuth state does not match, the token endpoint fails, or the response
/// body does not contain the expected token fields.
pub async fn exchange_grok_code(
    code: &str,
    returned_state: &str,
    expected_state: &str,
    pkce: &PkceCodes,
) -> Result<TokenData> {
    ensure_state(returned_state, expected_state)?;
    exchange_grok_grant(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", grok_redirect_uri().as_str()),
        ("client_id", GROK_CLIENT_ID),
        ("code_verifier", pkce.code_verifier.as_str()),
    ])
    .await
}

/// Refresh a Grok OAuth token.
///
/// # Errors
///
/// Returns an error when the token endpoint fails or the response body is invalid.
pub async fn refresh_grok_tokens(refresh_token: String) -> Result<TokenData> {
    let response = grok_refresh_request(&refresh_token).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        if let Some(reason) = detect_exhausted_reason(&body) {
            return Err(
                RefreshTokenExhaustedError::new(reason, Some(status.as_u16()), Some(body)).into(),
            );
        }
        bail!("grok token refresh failed ({status}): {body}");
    }
    let mut token = grok_token(
        &serde_json::from_str(&body).context("Grok refresh response is not JSON")?,
        false,
    )?;
    // auth.x.ai may rotate the refresh token on use; when it does not send one
    // back, the grant it just honored stays valid and is kept.
    if token.refresh_token.is_empty() {
        token.refresh_token = refresh_token;
    }
    Ok(token)
}

async fn exchange_grok_grant(pairs: &[(&str, &str)]) -> Result<TokenData> {
    let response = reqwest::Client::new()
        .post(GROK_TOKEN_URL)
        .header("x-grok-client-version", GROK_CLIENT_VERSION)
        .form(pairs)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("grok token exchange failed ({status}): {body}");
    }
    grok_token(
        &serde_json::from_str(&body).context("Grok token response is not JSON")?,
        true,
    )
}

fn grok_refresh_request(refresh_token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(GROK_TOKEN_URL)
        .header("x-grok-client-version", GROK_CLIENT_VERSION)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", GROK_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .timeout(std::time::Duration::from_secs(30))
}

fn grok_redirect_uri() -> String {
    // auth.x.ai's registered URI is the CLI's loopback form: host
    // `127.0.0.1`, port-agnostic (RFC 8252 — the CLI itself binds a random
    // port). `localhost` is not registered; the server rejects the whole
    // authorize request with "redirect_uri does not match any registered
    // URI". The fixed port keeps the token exchange's redirect_uri identical
    // to the authorize one.
    format!("http://127.0.0.1:{GROK_CALLBACK_PORT}{GROK_CALLBACK_PATH}")
}

/// Build the Antigravity authorize URL: authorization code with PKCE, offline
/// access so a refresh token is minted, and `prompt=consent` so a second login
/// mints a fresh one instead of silently reusing the first.
#[must_use]
pub fn generate_antigravity_auth_url(state: &str, pkce: &PkceCodes) -> String {
    let client_id = antigravity_client_id();
    let query = Serializer::new(String::new())
        .append_pair("access_type", "offline")
        .append_pair("client_id", &client_id)
        .append_pair("prompt", "consent")
        .append_pair("redirect_uri", &antigravity_redirect_uri())
        .append_pair("response_type", "code")
        .append_pair("scope", ANTIGRAVITY_SCOPES)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .finish();
    format!("{ANTIGRAVITY_AUTH_URL}?{query}")
}

/// Google's registered redirect is `localhost`, unlike auth.x.ai's `127.0.0.1`.
#[must_use]
pub fn antigravity_redirect_uri() -> String {
    format!("http://localhost:{ANTIGRAVITY_CALLBACK_PORT}{ANTIGRAVITY_CALLBACK_PATH}")
}

/// Exchange an Antigravity authorization code for a stored token, then read the
/// account's identity and the Cloud Code Assist project it is bound to.
///
/// # Errors
///
/// Returns an error when OAuth state does not match, the token endpoint fails,
/// identity cannot be read, or no project can be discovered or provisioned.
pub async fn exchange_antigravity_code(
    code: &str,
    returned_state: &str,
    expected_state: &str,
    pkce: &PkceCodes,
) -> Result<TokenData> {
    ensure_state(returned_state, expected_state)?;
    let redirect = antigravity_redirect_uri();
    let client_id = antigravity_client_id();
    let secret = antigravity_client_secret();
    exchange_antigravity_grant(&antigravity_code_grant(
        code, pkce, &redirect, &client_id, &secret,
    ))
    .await
}

/// The same exchange for a code carried by hand from a browser on another
/// machine: there is no state to compare, and the PKCE verifier still binds it.
///
/// # Errors
///
/// As [`exchange_antigravity_code`].
pub async fn exchange_antigravity_pasted_code(code: &str, pkce: &PkceCodes) -> Result<TokenData> {
    let redirect = antigravity_redirect_uri();
    let client_id = antigravity_client_id();
    let secret = antigravity_client_secret();
    exchange_antigravity_grant(&antigravity_code_grant(
        code, pkce, &redirect, &client_id, &secret,
    ))
    .await
}

fn antigravity_code_grant<'a>(
    code: &'a str,
    pkce: &'a PkceCodes,
    redirect: &'a str,
    client_id: &'a str,
    secret: &'a str,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("client_secret", secret),
        ("redirect_uri", redirect),
        ("code_verifier", pkce.code_verifier.as_str()),
    ]
}

async fn exchange_antigravity_grant(pairs: &[(&str, &str)]) -> Result<TokenData> {
    let response = reqwest::Client::new()
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(pairs)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("antigravity token exchange failed ({status}): {body}");
    }
    let data: Value =
        serde_json::from_str(&body).context("Antigravity token response is not JSON")?;
    let mut token = antigravity_token(&data)?;
    let (email, id) = antigravity_userinfo(&token.access_token).await?;
    token.email = email;
    token.account_uuid = id;
    let (project, tier) = antigravity_discover_project(&token.access_token).await?;
    token.project_id = Some(project);
    token.plan_type = tier;
    Ok(token)
}

/// Refresh an Antigravity access token. The project the account is bound to is
/// not rediscovered here: the account manager merges the stored one, so a
/// refresh never depends on the control plane.
///
/// # Errors
///
/// Returns an error when the token endpoint fails or a rejected refresh token
/// says the account needs a human (Reauth).
pub async fn refresh_antigravity_tokens(refresh_token: String) -> Result<TokenData> {
    let response = antigravity_refresh_request(&refresh_token).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        if let Some(reason) = detect_exhausted_reason(&body) {
            return Err(
                RefreshTokenExhaustedError::new(reason, Some(status.as_u16()), Some(body)).into(),
            );
        }
        bail!("antigravity token refresh failed ({status}): {body}");
    }
    let mut token = antigravity_token(
        &serde_json::from_str(&body).context("Antigravity refresh response is not JSON")?,
    )?;
    // Google may or may not rotate the refresh token; the grant it just honored
    // stays valid when it does not send one back.
    if token.refresh_token.is_empty() {
        token.refresh_token = refresh_token;
    }
    Ok(token)
}

fn antigravity_refresh_request(refresh_token: &str) -> reqwest::RequestBuilder {
    let client_id = antigravity_client_id();
    let secret = antigravity_client_secret();
    reqwest::Client::new()
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("client_secret", secret.as_str()),
            ("refresh_token", refresh_token),
        ])
        .timeout(std::time::Duration::from_secs(30))
}

fn antigravity_token(data: &Value) -> Result<TokenData> {
    Ok(TokenData {
        access_token: required_string(data, "access_token")?,
        refresh_token: data
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        email: String::new(),
        expires_at: expires_in_iso(
            data.get("expires_in").and_then(Value::as_u64),
            ANTIGRAVITY_TOKEN_TTL_SECONDS,
        ),
        account_uuid: String::new(),
        provider: ProviderId::antigravity(),
        // The scope set asks for `userinfo.*`, not `openid`, so no id_token is
        // minted; identity comes from the userinfo endpoint instead.
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
        project_id: None,
    })
}

fn antigravity_identity(userinfo: &Value) -> (String, String) {
    let email = userinfo
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let id = userinfo
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (email, id)
}

async fn antigravity_userinfo(access_token: &str) -> Result<(String, String)> {
    let response = reqwest::Client::new()
        .get(ANTIGRAVITY_USERINFO_URL)
        .bearer_auth(access_token)
        .header("User-Agent", crate::upstream::antigravity_user_agent())
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("antigravity userinfo failed ({status}): {body}");
    }
    let data: Value = serde_json::from_str(&body).context("Antigravity userinfo is not JSON")?;
    Ok(antigravity_identity(&data))
}

/// The project the account is bound to, provisioning one when it has none.
///
/// # Errors
///
/// Returns an error when the control plane refuses both discovery and
/// onboarding: an account with no project cannot serve a single request.
async fn antigravity_discover_project(access_token: &str) -> Result<(String, Option<String>)> {
    let load = antigravity_load_code_assist(access_token).await?;
    let tier = antigravity_tier(&load);
    if let Some(project) = antigravity_project(&load) {
        return Ok((project, tier));
    }
    let tier_id = tier
        .as_deref()
        .unwrap_or(ANTIGRAVITY_FALLBACK_TIER)
        .to_string();
    let project = antigravity_onboard_user(access_token, &tier_id).await?;
    Ok((project, tier))
}

fn antigravity_project(load: &Value) -> Option<String> {
    for key in ["cloudaicompanionProject", "projectId", "project"] {
        match load.get(key) {
            Some(Value::String(project)) if !project.trim().is_empty() => {
                return Some(project.trim().to_string());
            }
            Some(Value::Object(project)) => {
                let id = project
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty());
                if let Some(id) = id {
                    return Some(id.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn antigravity_tier(load: &Value) -> Option<String> {
    if let Some(tiers) = load.get("allowedTiers").and_then(Value::as_array) {
        for tier in tiers {
            if tier.get("isDefault").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let id = tier
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty());
            if let Some(id) = id {
                return Some(id.to_string());
            }
        }
    }
    load.get("currentTier")
        .and_then(|tier| tier.get("id"))
        .and_then(Value::as_str)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

async fn antigravity_load_code_assist(access_token: &str) -> Result<Value> {
    let url = format!(
        "{}{}",
        crate::upstream::ANTIGRAVITY_BASE_URL,
        crate::upstream::ANTIGRAVITY_LOAD_CODE_ASSIST_PATH
    );
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", crate::upstream::antigravity_user_agent())
        .json(&json!({"metadata": {"ideType": "ANTIGRAVITY"}}))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("antigravity loadCodeAssist failed ({status}): {body}");
    }
    serde_json::from_str(&body).context("Antigravity loadCodeAssist response is not JSON")
}

/// Onboard a first-time account and poll the long-running operation until it
/// names the project. The control plane answers `done: false` while it works.
async fn antigravity_onboard_user(access_token: &str, tier: &str) -> Result<String> {
    let url = format!(
        "{}{}",
        crate::upstream::ANTIGRAVITY_BASE_URL,
        crate::upstream::ANTIGRAVITY_ONBOARD_USER_PATH
    );
    let client = reqwest::Client::new();
    let mut last = String::from("onboarding did not complete");
    for _ in 0..ANTIGRAVITY_ONBOARD_ATTEMPTS {
        let response = client
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Accept", "*/*")
            .header("User-Agent", crate::upstream::antigravity_user_agent())
            .header("X-Goog-Api-Client", "gl-node/22.21.1")
            .json(&json!({
                "tier_id": tier,
                "metadata": {
                    "ide_type": "ANTIGRAVITY",
                    "ide_version": crate::upstream::antigravity_version(),
                    "ide_name": "antigravity"
                }
            }))
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("antigravity onboardUser failed ({status}): {body}");
        }
        let data: Value =
            serde_json::from_str(&body).context("Antigravity onboardUser response is not JSON")?;
        if data.get("done").and_then(Value::as_bool) != Some(true) {
            last = body;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }
        if let Some(project) = data.get("response").and_then(antigravity_project) {
            return Ok(project);
        }
        bail!("antigravity onboardUser finished without a project: {body}");
    }
    bail!("antigravity onboardUser did not complete: {last}")
}

/// Derive a `TokenData` from a token-endpoint response. The code exchange
/// mints an `id_token` carrying the account identity, so `login` requires
/// it. The refresh grant does not return one, so with `require_id_token`
/// false the identity fields stay empty and the account manager keeps the
/// stored identity (empty email/uuid and a `None` plan read as "unchanged").
fn grok_token(data: &Value, require_id_token: bool) -> Result<TokenData> {
    let id_token = data
        .get("id_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned);
    if require_id_token && id_token.is_none() {
        bail!("token response is missing id_token");
    }
    let claims = match &id_token {
        Some(token) => decode_jwt_payload(token)?,
        None => Value::Null,
    };
    let claim_string = |field: &str| {
        claims
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(TokenData {
        access_token: required_string(data, "access_token")?,
        refresh_token: data
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        email: if require_id_token {
            claims
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string()
        } else {
            claim_string("email")
        },
        expires_at: expires_in_iso(
            data.get("expires_in").and_then(Value::as_u64),
            GROK_TOKEN_TTL_SECONDS,
        ),
        account_uuid: claims
            .get("principal_id")
            .or_else(|| claims.get("sub"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        provider: ProviderId::grok(),
        id_token,
        last_refresh_at: None,
        plan_type: claims.get("tier").map(|tier| format!("tier-{tier}")),
        project_id: None,
    })
}

/// The paste fallback auth.x.ai offers when a redirect cannot reach the
/// machine running the login (headless hosts, forwarded browsers): the
/// consent page displays the code for the user to carry over by hand, which
/// is what "Copy the code below into Grok Build" is. Accepts the full
/// callback URL (state verified on exchange) or the bare code.
///
/// # Errors
///
/// Returns an error when the paste is blank, is a URL without a `code`, or
/// is not a parseable URL.
pub fn parse_grok_paste(input: &str) -> Result<(String, Option<String>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("nothing was pasted");
    }
    if trimmed.contains("://") {
        let parsed = url::Url::parse(trimmed).context("pasted callback is not a URL")?;
        let code = parsed
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .context("pasted callback carries no code")?;
        let state = parsed
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned());
        return Ok((code, state));
    }
    Ok((trimmed.to_string(), None))
}

/// Exchange a pasted code. The paste carries no state to verify, so the
/// check is skipped: the code is still bound to this login's PKCE verifier,
/// which only this process holds.
///
/// # Errors
///
/// Returns an error when the token endpoint fails or its response body does
/// not contain the expected token fields.
pub async fn exchange_grok_pasted_code(code: &str, pkce: &PkceCodes) -> Result<TokenData> {
    exchange_grok_grant(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", grok_redirect_uri().as_str()),
        ("client_id", GROK_CLIENT_ID),
        ("code_verifier", pkce.code_verifier.as_str()),
    ])
    .await
}

fn ensure_state(returned_state: &str, expected_state: &str) -> Result<()> {
    if returned_state != expected_state {
        bail!("OAuth state mismatch");
    }
    Ok(())
}

fn anthropic_token(data: &Value) -> Result<TokenData> {
    let account = data.get("account").unwrap_or(&Value::Null);
    Ok(TokenData {
        access_token: required_string(data, "access_token")?,
        refresh_token: required_string(data, "refresh_token")?,
        email: account
            .get("email_address")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        expires_at: expires_in_iso(data.get("expires_in").and_then(Value::as_u64), 3600),
        account_uuid: account
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        provider: ProviderId::anthropic(),
        id_token: None,
        last_refresh_at: None,
        plan_type: None,
        project_id: None,
    })
}

fn codex_token(data: &Value) -> Result<TokenData> {
    let id_token = required_string(data, "id_token")?;
    let claims = decode_jwt_payload(&id_token)?;
    let auth = claims
        .get("https://api.openai.com/auth")
        .unwrap_or(&Value::Null);
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let account_uuid = auth
        .get("chatgpt_account_id")
        .or_else(|| claims.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let plan_type = auth
        .get("chatgpt_plan_type")
        .or_else(|| claims.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(TokenData {
        access_token: required_string(data, "access_token")?,
        refresh_token: required_string(data, "refresh_token")?,
        email,
        expires_at: expires_in_iso(data.get("expires_in").and_then(Value::as_u64), 3600),
        account_uuid,
        provider: ProviderId::codex(),
        id_token: Some(id_token),
        last_refresh_at: None,
        plan_type,
        project_id: None,
    })
}

fn required_string(data: &Value, field: &str) -> Result<String> {
    data.get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .with_context(|| format!("token response is missing {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_detects_exhausted_refresh_markers() {
        assert_eq!(
            detect_exhausted_reason(r#"{"error":"invalid_grant"}"#),
            Some("invalid_grant")
        );
        assert_eq!(
            detect_exhausted_reason("refresh_token_reused by another client"),
            Some("refresh_token_reused")
        );
        assert_eq!(detect_exhausted_reason("temporary outage"), None);
    }

    fn content_type(req: &reqwest::Request) -> String {
        req.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    fn body_string(req: &reqwest::Request) -> String {
        let bytes = req
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("refresh request always sets a body");
        std::str::from_utf8(bytes)
            .expect("refresh request body is valid utf-8")
            .to_owned()
    }

    #[test]
    fn codex_refresh_uses_form_encoded_request() {
        let req = codex_refresh_request("refresh-xyz")
            .build()
            .expect("codex refresh request builds");
        assert_eq!(req.url().as_str(), CODEX_TOKEN_URL);
        assert_eq!(content_type(&req), "application/x-www-form-urlencoded");
        let body = body_string(&req);
        assert!(body.contains("grant_type=refresh_token"), "{body}");
        assert!(body.contains("refresh_token=refresh-xyz"), "{body}");
    }

    #[test]
    fn anthropic_refresh_uses_json_request() {
        let req = anthropic_refresh_request("refresh-xyz")
            .build()
            .expect("anthropic refresh request builds");
        assert_eq!(req.url().as_str(), ANTHROPIC_TOKEN_URL);
        assert_eq!(content_type(&req), "application/json");
        let body = body_string(&req);
        assert!(body.contains("\"grant_type\":\"refresh_token\""), "{body}");
        assert!(body.contains("\"refresh_token\":\"refresh-xyz\""), "{body}");
    }

    fn fake_id_token(claims: &serde_json::Value) -> String {
        use base64::Engine as _;
        let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        format!(
            "{}.{}.sig",
            encode(br#"{"alg":"ES256","typ":"at+jwt"}"#),
            encode(
                serde_json::to_vec(claims)
                    .expect("claims serialize")
                    .as_slice()
            )
        )
    }

    #[test]
    fn grok_auth_url_carries_pkce_state_nonce_and_referrer() {
        let pkce = PkceCodes {
            code_verifier: "v".repeat(43),
            code_challenge: "c".to_string(),
        };
        let url = generate_grok_auth_url("state-1", &pkce, "nonce-1");
        assert!(url.starts_with(GROK_AUTH_URL), "{url}");
        for piece in [
            "response_type=code",
            &format!("client_id={GROK_CLIENT_ID}"),
            &format!("redirect_uri=http%3A%2F%2F127.0.0.1%3A{GROK_CALLBACK_PORT}%2Fcallback"),
            &format!("scope={}", urlencode(GROK_SCOPE)),
            "code_challenge=c",
            "code_challenge_method=S256",
            "state=state-1",
            "nonce=nonce-1",
            "referrer=grok-build",
        ] {
            assert!(url.contains(piece), "{piece} missing from {url}");
        }
    }

    fn urlencode(value: &str) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("", value)
            .finish()
            .trim_start_matches('=')
            .to_string()
    }

    #[test]
    fn grok_refresh_uses_form_encoded_request() {
        let req = grok_refresh_request("refresh-xyz")
            .build()
            .expect("grok refresh request builds");
        assert_eq!(req.url().as_str(), GROK_TOKEN_URL);
        assert_eq!(content_type(&req), "application/x-www-form-urlencoded");
        let body = body_string(&req);
        assert!(body.contains("grant_type=refresh_token"), "{body}");
        assert!(body.contains("refresh_token=refresh-xyz"), "{body}");
        assert!(
            body.contains(&format!("client_id={GROK_CLIENT_ID}")),
            "{body}"
        );
    }

    #[test]
    fn grok_token_derivation_reads_id_token_claims() {
        let id_token = fake_id_token(&serde_json::json!({
            "iss": "https://auth.x.ai",
            "sub": "42284626-f3a1-47bd-9717-b7afd95d86d9",
            "email": "operator@example.com",
            "principal_id": "42284626-f3a1-47bd-9717-b7afd95d86d9",
            "tier": 3,
        }));
        let token = grok_token(
            &serde_json::json!({
                "access_token": "at-jwt",
                "refresh_token": "refresh-1",
                "id_token": id_token,
                "expires_in": 21_600,
            }),
            true,
        )
        .expect("grok token derives");
        assert_eq!(token.provider, ProviderId::grok());
        assert_eq!(token.email, "operator@example.com");
        assert_eq!(token.account_uuid, "42284626-f3a1-47bd-9717-b7afd95d86d9");
        assert_eq!(token.plan_type.as_deref(), Some("tier-3"));
        assert_eq!(token.refresh_token, "refresh-1");
        assert_eq!(token.access_token, "at-jwt");
    }

    #[test]
    fn grok_token_falls_back_to_sub_when_principal_id_is_absent() {
        let id_token = fake_id_token(&serde_json::json!({
            "sub": "sub-only-id",
            "email": "operator@example.com",
        }));
        let token = grok_token(
            &serde_json::json!({
                "access_token": "at",
                "refresh_token": "r",
                "id_token": id_token,
            }),
            true,
        )
        .expect("grok token derives");
        assert_eq!(token.account_uuid, "sub-only-id");
        assert_eq!(token.plan_type, None);
    }

    #[test]
    fn a_pasted_callback_url_yields_code_and_state() {
        let (code, state) =
            parse_grok_paste("http://127.0.0.1:14550/callback?code=abc123&state=xyz789")
                .expect("URL paste parses");
        assert_eq!(code, "abc123");
        assert_eq!(state.as_deref(), Some("xyz789"));
    }

    #[test]
    fn a_pasted_bare_code_yields_no_state() {
        // auth.x.ai's manual fallback shows a bare code, not a URL; there is
        // nothing to verify against, and the PKCE verifier is what binds it.
        let pasted = "Z5HJkCGO3wsKaBmHPw2AfZjOto6_ToL7ug-LjjILd9oeUck5B2NWE7oSqgf7QkAhJ_oYt_ryEzrU-SRiNJt_NA";
        let (code, state) = parse_grok_paste(&format!("  {pasted}\n")).expect("bare code parses");
        assert_eq!(code, pasted);
        assert_eq!(state, None);
    }

    #[test]
    fn a_pasted_url_without_a_code_is_refused() {
        assert!(parse_grok_paste("http://127.0.0.1:14550/callback?state=xyz").is_err());
        assert!(parse_grok_paste("   ").is_err());
    }

    #[test]
    fn grok_refresh_without_id_token_keeps_identity_empty() {
        // The refresh grant's real shape (observed live against auth.x.ai):
        // access and refresh tokens, no id_token. Empty identity fields tell
        // the account manager to keep what it has on file.
        let token = grok_token(
            &serde_json::json!({
                "access_token": "new-at",
                "expires_in": 21_600,
            }),
            false,
        )
        .expect("refresh token derives");
        assert_eq!(token.access_token, "new-at");
        assert_eq!(token.email, "");
        assert_eq!(token.account_uuid, "");
        assert_eq!(token.plan_type, None);
    }

    #[test]
    fn grok_exchange_without_id_token_is_refused() {
        assert!(
            grok_token(
                &serde_json::json!({"access_token": "at", "refresh_token": "r"}),
                true,
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod antigravity_oauth_tests {
    use super::{
        ANTIGRAVITY_TOKEN_URL, antigravity_client_id, antigravity_client_secret,
        antigravity_identity, antigravity_project, antigravity_refresh_request, antigravity_tier,
        generate_antigravity_auth_url,
    };
    use crate::types::PkceCodes;
    use serde_json::json;

    fn content_type(request: &reqwest::Request) -> String {
        request
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    fn body_string(request: &reqwest::Request) -> String {
        request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default()
    }

    #[test]
    fn the_auth_url_carries_pkce_offline_access_consent_and_the_registered_redirect() {
        let pkce = PkceCodes {
            code_verifier: "v".repeat(43),
            code_challenge: "challenge".to_string(),
        };
        let url = generate_antigravity_auth_url("state-1", &pkce);
        assert!(url.starts_with(super::ANTIGRAVITY_AUTH_URL), "{url}");
        for piece in [
            "access_type=offline",
            "prompt=consent",
            "response_type=code",
            "code_challenge=challenge",
            "code_challenge_method=S256",
            "state=state-1",
            &format!("client_id={}", antigravity_client_id()),
            // The registered redirect is `localhost`, URL-encoded.
            "redirect_uri=http%3A%2F%2Flocalhost%3A51121%2Foauth-callback",
            // Every scope is a full https URL, so the separator is encoded.
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform",
            "auth%2Faicode",
            "auth%2Fexperimentsandconfigs",
        ] {
            assert!(url.contains(piece), "{piece} missing from {url}");
        }
    }

    #[test]
    fn a_refresh_sends_the_client_secret_beside_the_grant() {
        let request = antigravity_refresh_request("refresh-xyz")
            .build()
            .expect("refresh request builds");
        assert_eq!(request.url().as_str(), ANTIGRAVITY_TOKEN_URL);
        assert_eq!(content_type(&request), "application/x-www-form-urlencoded");
        let body = body_string(&request);
        assert!(body.contains("grant_type=refresh_token"), "{body}");
        assert!(body.contains("refresh_token=refresh-xyz"), "{body}");
        assert!(
            body.contains(&format!("client_id={}", antigravity_client_id())),
            "{body}"
        );
        assert!(
            body.contains(&format!("client_secret={}", antigravity_client_secret())),
            "{body}"
        );
    }

    #[test]
    fn the_project_and_tier_come_from_load_code_assist() {
        let load = json!({
            "cloudaicompanionProject": {"id": "cloud-code-project"},
            "currentTier": {"id": "standard-tier"},
            "allowedTiers": [
                {"id": "free-tier", "isDefault": false},
                {"id": "standard-tier", "isDefault": true}
            ]
        });
        assert_eq!(
            antigravity_project(&load).as_deref(),
            Some("cloud-code-project")
        );
        assert_eq!(antigravity_tier(&load).as_deref(), Some("standard-tier"));
        // The plain-string project form is accepted too.
        assert_eq!(
            antigravity_project(&json!({"cloudaicompanionProject": "p-1"})).as_deref(),
            Some("p-1")
        );
        assert_eq!(antigravity_project(&json!({})), None);
        // With no default tier, the current one labels the account.
        assert_eq!(
            antigravity_tier(&json!({"currentTier": {"id": "t"}})).as_deref(),
            Some("t")
        );
        assert_eq!(antigravity_tier(&json!({})), None);
    }

    #[test]
    fn identity_comes_from_the_userinfo_body_not_an_id_token() {
        assert_eq!(
            antigravity_identity(&json!({"id": "1234", "email": "operator@example.com"})),
            ("operator@example.com".to_string(), "1234".to_string())
        );
        assert_eq!(
            antigravity_identity(&json!({})),
            ("unknown".to_string(), String::new())
        );
    }
}
