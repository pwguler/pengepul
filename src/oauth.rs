use url::form_urlencoded::Serializer;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::types::{PkceCodes, ProviderId, RefreshTokenExhaustedError, TokenData};
use crate::upstream::GROK_CLIENT_VERSION;
use crate::utils::{decode_jwt_payload, expires_in_iso};

pub const ANTHROPIC_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
pub const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const ANTHROPIC_REDIRECT_URI: &str = "http://localhost:54545/callback";
pub const ANTHROPIC_SCOPE: &str = "org:create_api_key user:profile user:inference";

pub const CODEX_ISSUER: &str = "https://auth.openai.com";
pub const CODEX_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_CALLBACK_PATH: &str = "/auth/callback";
pub const CODEX_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub const ANTHROPIC_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
pub const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

pub const GROK_ISSUER: &str = "https://auth.x.ai";
pub const GROK_AUTH_URL: &str = "https://auth.x.ai/oauth2/authorize";
pub const GROK_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const GROK_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const GROK_CALLBACK_PORT: u16 = 14550;
pub const GROK_CALLBACK_PATH: &str = "/callback";
pub const GROK_SCOPE: &str = "openid profile email offline_access grok-cli:access";
pub const GROK_REFERRER: &str = "grok-build";
/// Grok access tokens live 6 h (21600 s by the minted JWT's `exp - iat`).
pub const GROK_TOKEN_TTL_SECONDS: u64 = 21_600;

#[must_use]
pub fn detect_exhausted_reason(body: &str) -> Option<&'static str> {
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
    format!("http://localhost:{GROK_CALLBACK_PORT}{GROK_CALLBACK_PATH}")
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
    })
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
            &format!("redirect_uri=http%3A%2F%2Flocalhost%3A{GROK_CALLBACK_PORT}%2Fcallback"),
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
