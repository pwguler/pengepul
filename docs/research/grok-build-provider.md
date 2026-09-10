# Grok Build as a pengepul provider

Researched 2026-09-10 against the grok-build source (xai-org/grok-build @
`37949780c144e37df692e3d669051a21fec24f20`) and a live probe against the
production relay using a real subscription token. Every protocol claim below is
from source or verified on the wire; the two items that are not are marked
unverified.

## Question

Can pengepul pool grok.com subscriptions the way it pools Anthropic and
ChatGPT/Codex accounts — by driving grok build's own OAuth — and what would the
provider cost to build?

## Answer

Yes, and it is the cheapest OAuth provider yet: standard OIDC discovery, a
public PKCE client (no secret, no org headers), and an upstream that speaks
plain OpenAI Chat Completions. The relay is
`https://cli-chat-proxy.grok.com/v1`, and a session token works against it with
three headers, verified end-to-end with a live `pong` exchange. Building
`pengepul login --provider grok` is the same shape as codex, minus codex's
special cases. Registering `https://api.x.ai/v1` as a keyed provider already
works today with zero code, but draws from console credits, not a subscription —
it is the stopgap, not the goal.

## The two auth paths

Grok build (xAI's coding CLI) resolves credentials in this order: per-model
key, active session token from `~/.grok/auth.json`, then `XAI_API_KEY`
(`crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md`). The two
paths relevant to pengepul:

- **OAuth session token** — `grok login` runs authorization code + PKCE against
  `auth.x.ai`. The token draws from the grok.com subscription (the local token's
  JWT carries `tier: 3`). This is what pengepul should pool.
- **API key** — `xai-...` from console.x.ai against `https://api.x.ai/v1`,
  pay-per-token, plain OpenAI-compatible. Works in pengepul today:

  ```sh
  pengepul login --provider xai \
    --base-url https://api.x.ai/v1 --key $XAI_API_KEY
  ```

## The relay, verified on the wire

The CLI's production endpoint is
`https://cli-chat-proxy.grok.com/v1` (`crates/codegen/xai-grok-env/src/lib.rs`,
`PROD_CLI_CHAT_PROXY_BASE_URL`; overridable via
`GROK_CLI_CHAT_PROXY_BASE_URL` / `[endpoints] cli_chat_proxy_base_url`).
`crates/codegen/xai-grok-shell-base/src/util/mod.rs` pins the chat URL as
`.../v1/chat/completions`. The default models are `grok-4.6` and `grok-4.5`
(500k context) with `api_backend: "responses"`
(`crates/codegen/xai-grok-models/default_models.json`); the sampler also
speaks OpenAI Chat Completions
(`crates/codegen/xai-grok-sampler/src/attribution.rs`).

Live probe (2026-09-10, subscription token from `~/.grok/auth.json`):

```
POST https://cli-chat-proxy.grok.com/v1/chat/completions
Authorization: Bearer <session token>
X-XAI-Token-Auth: xai-grok-cli
x-grok-client-version: 0.1.202
Content-Type: application/json

{"model":"grok-4.6","messages":[{"role":"user","content":"reply exactly: pong"}],
 "max_tokens":16,"stream":false}
```

→ `HTTP 200`, standard `chat.completion` body. Observed response shape:

- served `model` is `grok-4.6-build` (the proxy maps the requested model to the
  build-tuned variant)
- assistant message carries a `reasoning_content` string beside `content`
- `usage` includes `reasoning_tokens`, `cached_tokens`, and
  `usage.cost_in_usd_ticks` (tick scale unverified)

The two headers besides the bearer are load-bearing:

- `X-XAI-Token-Auth: xai-grok-cli` — routes the server's nginx auth subrequest
  to OAuth (`crates/codegen/xai-grok-shell/src/remote/client.rs`:
  "Must include X-XAI-Token-Auth so nginx auth subrequest routes to OAuth";
  set wherever `GrokAuthCredentials::apply` attaches the user token,
  `crates/codegen/xai-grok-login/src/grok_auth_credentials.rs`).
- `x-grok-client-version: 0.1.202` — the proxy enforces a minimum CLI version
  and answers `HTTP 426` with
  `{"error":"Your Grok CLI version (none) is outdated. Please update to
  version 0.1.202 or later ..."}` without it. Verified: no header → 426;
  `0.1.202` → 200. A `User-Agent` alone is not read for this gate.

The CLI itself stamps the version from
`crates/codegen/xai-grok-version/src/lib.rs` (`VERSION`, release-injected via
`GROK_VERSION`).

## Login flow

`grok login` (first-party) is the generic OIDC machinery pointed at xAI's own
issuer: `OAuth2ProviderConfig::as_oidc()` reuses the OIDC flow
(`crates/codegen/xai-grok-login/src/config.rs`), and the production client_id
is hardcoded, obfstr-obfuscated:
`b1a00492-073a-47ea-816f-4c329264a828` (same crate, `Default for
GrokComConfig`). Confirmed against the local `~/.grok/auth.json`: the scope key
is `https://auth.x.ai::b1a00492-...` and the access token's `iss`/`aud` carry
both values.

Endpoints — from the live discovery document
`https://auth.x.ai/.well-known/openid-configuration` (the CLI discovers the
same way, `crates/codegen/xai-grok-login/src/oidc/protocol.rs:278`):

| Piece | Value |
|---|---|
| issuer | `https://auth.x.ai` |
| authorize | `https://auth.x.ai/oauth2/authorize` |
| token | `https://auth.x.ai/oauth2/token` |
| device authorization | `https://auth.x.ai/oauth2/device/code` |
| revocation | `https://auth.x.ai/oauth2/revoke` |
| jwks | `https://auth.x.ai/.well-known/jwks.json` (id tokens are ES256) |

Flow mechanics (`crates/codegen/xai-grok-login/src/oidc/login.rs`,
`.../oidc/protocol.rs`):

- Authorization code + PKCE S256; `state` and `nonce` are UUIDv7 strings.
- The CLI binds `127.0.0.1` on a random port (port `0`) and uses
  `http://127.0.0.1:<port>/callback` as the redirect URI; it also accepts a
  pasted callback URL, so a fixed port works too.
- Authorize URL query: `response_type=code&client_id=...&redirect_uri=...&
  scope=...&code_challenge=...&code_challenge_method=S256&state=...&nonce=...&
  referrer=grok-build` (`referrer` comes from `DEFAULT_OAUTH2_REFERRER`,
  overridable per request; `build_authorize_url` also takes optional
  `principal_type`/`principal_id` for team logins — not needed for personal
  accounts).
- Scopes requested (`default_oauth2_scopes`): `openid profile email
  offline_access grok-cli:access`. The server grants more: the live token's
  scope also includes `api:access conversations:read conversations:write
  workspaces:read workspaces:write`.
- Token exchange: form-encoded
  `grant_type=authorization_code&code=...&redirect_uri=...&client_id=...&
  code_verifier=...` (no client secret), plus the
  `x-grok-client-version` header. Response: `access_token`,
  `refresh_token`, `id_token`, `expires_in`.
- Refresh: `grant_type=refresh_token&client_id=...&refresh_token=...`; retry on
  5xx/429, stop on `invalid_grant`/`invalid_client`
  (`.../oidc/protocol.rs`, `is_transient_refresh_error`).
- Device-code flow exists for headless logins:
  `POST {issuer}/oauth2/device/code`, display `verification_uri` +
  `user_code`, poll the token endpoint (`.../device_code.rs`).

Token facts from the live credential:

- Access token is an `at+jwt` (ES256, kid `oauth2-production-...`) with
  `iss https://auth.x.ai`, `aud` = client_id, `principal_type: "User"`,
  `principal_id`, `team_id`, `tier`, and the scope list. Lifetime is 21600 s
  (6 h) by `exp − iat`.
- Refresh token is long-lived; docs state a 30-day fallback lifetime when the
  server omits an expiry (`02-authentication.md`).
- `auth.json` layout: one object per scope key (`<issuer>::<client_id>`),
  fields `key`, `auth_mode: "oidc"`, `create_time`, `user_id`, `email`,
  `principal_type`, `principal_id`, `team_id`, `refresh_token`, written `0600`
  (`crates/codegen/xai-grok-login/src/storage.rs`). Hot-reloaded by the CLI on
  change — pengepul can read the CLI's own file to import an existing login,
  though driving the flow itself (like anthropic/codex) is the cleaner fit for
  pooling.

## Pengepul integration map

Same skeleton as codex, minus the special cases (no originator param, no org
headers, standard OIDC):

- `src/types.rs` — `ProviderKind::Grok`, canonical id `grok`.
- `src/oauth.rs` — `GROK_ISSUER = "https://auth.x.ai"`, `GROK_CLIENT_ID`,
  `GROK_SCOPE`; `generate_grok_auth_url` (PKCE + state + nonce + referrer),
  `exchange_grok_code` (form-encoded), `refresh_grok_tokens` (form-encoded,
  like codex). `email`, `tier`, and account id come straight off the
  `id_token` JWT the code already decodes for codex.
- `src/upstream.rs` — grok upstream: base
  `https://cli-chat-proxy.grok.com/v1`, bearer = session token, plus the
  `X-XAI-Token-Auth` and `x-grok-client-version` headers. The cloaking
  machinery already does per-provider header overrides for codex
  (`cloaking_versions.rs`), so a pinned version header fits there; refresh on
  401 mirrors the codex path.
- `src/models.rs` — serve `grok-4.6` / `grok-4.5` (500k context, reasoning
  efforts `xhigh`/`high`/`medium`/`low`); the `xai/` namespace already exists
  for the openrouter passthrough.
- Token persistence under the grok provider dir follows the existing
  `TokenData` shape; 6 h access-token TTL means the refresh path runs often —
  cheap, but worth the same single-flight treatment codex gets.

## Gotchas

- **Client_id is a public client, hardcoded in the CLI** (obfstr-obfuscated).
  No secret to protect, same trust model as the anthropic/codex clients. But it
  is not machine-discoverable: the CLI's first-party config normally arrives
  via remote settings (`grok_oauth_enabled` in
  `crates/codegen/xai-grok-config-types/src/lib.rs`), so a rotation would ship
  silently in a CLI update. Pin it, and treat auth failures on a working
  config as a rotation signal.
- **Version floor moves.** The proxy demanded ≥ 0.1.202 at research time.
  A stale pin fails closed with a clean 426 naming the minimum, so detection
  is cheap: bump the pinned `x-grok-client-version` when that body changes.
- **`reasoning_content`** rides in the assistant message on chat-completions
  responses. Decide pass-through vs strip (the strip-thinking-suffix machinery
  already exists); OpenAI-style clients that don't expect the field will
  tolerate it, but it counts toward billed completion tokens.
- **`cost_in_usd_ticks`** is a new usage field, useful for the usage console
  once tick scale is confirmed (unverified).
- **Tokens are broad.** The server grants more scopes than requested
  (`api:access`, `conversations:*`, `workspaces:*`), so a leaked token is
  potent. 0600 under `~/.pengepul` matches the existing posture; don't log
  it.
- **BYOK leakage guard exists server-side by design:** the CLI only refreshes
  session tokens against `*.x.ai` endpoints
  (`crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs`),
  which is consistent with pengepul relaying grok through its own bearer, not
  re-exporting session tokens to clients.

## Sources

- grok-build source, `xai-org/grok-build` @
  `37949780c144e37df692e3d669051a21fec24f20`:
  `crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md`,
  `crates/codegen/xai-grok-env/src/lib.rs`,
  `crates/codegen/xai-grok-login/src/{config,model,storage,flow}.rs`,
  `crates/codegen/xai-grok-login/src/oidc/{login,protocol}.rs`,
  `crates/codegen/xai-grok-login/src/device_code.rs`,
  `crates/codegen/xai-grok-login/src/grok_auth_credentials.rs`,
  `crates/codegen/xai-grok-config-types/src/lib.rs`,
  `crates/codegen/xai-grok-sampler/src/{client,attribution}.rs`,
  `crates/codegen/xai-grok-shell-base/src/util/mod.rs`,
  `crates/codegen/xai-grok-shell/src/remote/client.rs`,
  `crates/codegen/xai-grok-models/default_models.json`,
  `crates/codegen/xai-grok-version/src/lib.rs`.
- Live discovery: `https://auth.x.ai/.well-known/openid-configuration`
  (fetched 2026-09-10).
- Live relay probe against
  `https://cli-chat-proxy.grok.com/v1/chat/completions`, 2026-09-10: no
  version header → 426; `0.1.202` → 200 with a chat.completion body.
- Local credential: `~/.grok/auth.json` (scope key, JWT claims, refresh token
  present; token values not reproduced here).
